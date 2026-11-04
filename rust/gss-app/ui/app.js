// UI state machine mirroring tkinter_app.py: two modes (IDLE/RUNNING(op)),
// worker events via Tauri, iid = `article-{storedIndex}` surviving filters,
// check/download write back by index, sync Excel/CSV exports, background
// citations. All strings verbatim from the Python tables.

const OPERATION_HEADINGS = {
  search: "Searching Google Scholar",
  check: "Checking metadata and public PDFs",
  import: "Importing journal ranking records",
  download: "Downloading selected public PDFs",
  citations: "Exporting citations",
};
const PDF_FILTERS = ["All", "Public PDF available", "No public PDF found", "Unknown"];
const NO_RANKING_NOTICE = "No ranks imported.";
const DETAIL_DISCLAIMER =
  "Journal quartiles and ranks describe the journal, not this paper. Ranking data comes from files you import, and an unmatched journal stays unknown rather than unranked. Public-PDF labels describe what the check actually verified at the time it ran. Links and metadata come from Google Scholar and may be incomplete or wrong. Nothing here replaces reading the source.";
const DEFAULT_EXPORT_NAMES = {
  excel: "scholar_articles.xlsx",
  csv: "scholar_articles.csv",
  bibtex: "scholar_articles.bib",
  ris: "scholar_articles.ris",
};

// Verbatim from tkinter_app.py status_message() (L201) and status_heading()
// (L219); the SUCCESS hint is appended at runtime, exactly as Python does.
const STATUS_DEFAULTS = {
  SUCCESS: "Search completed successfully.",
  PARTIAL_SUCCESS: "Search stopped early. Collected results were preserved.",
  NO_RESULTS: "No results found.",
  RATE_LIMITED: "Google Scholar temporarily limited requests.",
  BLOCKED: "Google Scholar returned a blocked or challenge page.",
  NETWORK_ERROR: "Network request failed.",
  PARSING_ERROR: "The response could not be interpreted safely.",
  CANCELLED: "Search was cancelled.",
};
const STATUS_HEADINGS = {
  SUCCESS: "Search complete",
  PARTIAL_SUCCESS: "Search stopped early",
  NO_RESULTS: "No results found",
  RATE_LIMITED: "Google Scholar temporarily limited requests",
  BLOCKED: "Google Scholar returned a challenge page",
  NETWORK_ERROR: "Network request failed",
  PARSING_ERROR: "Results could not be parsed safely",
  CANCELLED: "Search cancelled",
};
const CHECK_METADATA_HINT =
  " Run 'Check metadata / PDFs' to detect which papers have downloadable public PDFs; until then every PDF column shows Unknown.";
const STATUS_TONES = {
  SUCCESS: "success",
  PARTIAL_SUCCESS: "warning",
  CANCELLED: "warning",
  RATE_LIMITED: "warning",
  BLOCKED: "error",
  NETWORK_ERROR: "error",
  PARSING_ERROR: "error",
  NO_RESULTS: "error",
};

// ---- state ----
let running = false; // RUNNING(operation) vs IDLE
let currentOperation = null;
let storedArticles = []; // row identity = index
let currentRequest = null;
let pdfFilter = "All";
let summaryBase = "No search has run yet.";
let rankingCount = 0;
let currentYear = new Date().getFullYear();
const selection = new Set(); // stored indices in click order
let sortKey = null; // column key or null (source order)
let sortDir = 1; // 1 ascending, -1 descending
let resultsQuery = ""; // session-only free-text filter
let lastExportFolder = ""; // for the "Open folder" button
let ctxTarget = null; // stored index of the row context menu
let focusRow = -1; // stored index of the keyboard-focused grid row
let anchorRow = -1; // shift-range anchor for the grid
let lastRequest = null; // snapshot of the last accepted search
let detailIndex = -1; // record shown in the details dialog
let queryHistory = []; // recent successful queries, newest first
let historyCursor = -1; // recall position inside queryHistory
let hiddenColumns = new Set(); // column keys hidden by the Columns menu
const ALL_COLUMNS = ["title", "authors", "index", "rank", "pdf", "link", "cited", "oa"];

const $ = (id) => document.getElementById(id);

// Commands reject with { code, message }; older paths may still hand back a
// bare string, so both shapes are normalised here. The UI switches on `code`,
// never on message text.
function errCode(err) {
  return err && typeof err === "object" && "code" in err ? String(err.code) : "";
}
function errMessage(err) {
  if (err && typeof err === "object" && "message" in err) return String(err.message);
  return String(err);
}

function joinPath(folder, filename) {
  if (!folder) return filename;
  return folder.endsWith("\\") || folder.endsWith("/") ? folder + filename : folder + "\\" + filename;
}

// ---- year options ----
function yearFilterOptions(y) {
  return [
    { label: "Any time", from: null, to: null },
    { label: `Since ${y}`, from: y, to: null },
    { label: `Since ${y - 1}`, from: y - 1, to: null },
    { label: "Last five years", from: y - 4, to: null },
    { label: "Custom range", from: null, to: null, custom: true },
  ];
}
function yearRangeLabel(from, to) {
  if (from != null && to != null) return `Years: ${from}\u2013${to}`;
  if (from != null) return `Years: since ${from}`;
  if (to != null) return `Years: through ${to}`;
  return "Years: any time";
}

function init() {
  const preset = $("yearPreset");
  preset.innerHTML = "";
  for (const opt of yearFilterOptions(currentYear)) {
    const o = document.createElement("option");
    o.textContent = opt.label;
    preset.appendChild(o);
  }
  const filter = $("pdfFilter");
  filter.innerHTML = "";
  for (const label of PDF_FILTERS) {
    const o = document.createElement("option");
    o.textContent = label;
    filter.appendChild(o);
  }

  $("btnSearch").onclick = startSearch;
  $("btnCancel").onclick = cancelOperation;
  // Explicit wrapper: passing the handler directly would put the click event
  // into the metadataOnly parameter.
  $("btnCheck").onclick = () => checkMetadataPdfs(false);
  $("btnImport").onclick = importRanking;
  $("btnDownload").onclick = downloadSelected;
  $("btnDetails").onclick = openDetail;
  $("btnExportExcel").onclick = () => exportSpreadsheet("excel");
  $("btnExportCsv").onclick = () => exportSpreadsheet("csv");
  $("btnExportCitations").onclick = openCitationsDialog;
  $("btnBrowse").onclick = browseFolder;
  $("btnCloseDetail").onclick = () => $("detailDialog").close();
  $("btnCloseCitations").onclick = () => $("citationsDialog").close();
  $("btnDoExport").onclick = startCitationExport;
  $("pdfFilter").onchange = () => {
    renderResults();
    scheduleSave();
  };
  $("tableBody").onclick = onTableClick;
  $("resultsTable").addEventListener("keydown", onTableKeyDown);
  $("resultsTable").addEventListener("focus", () => {
    if (focusRow === -1) { const ids = visibleIndices(); if (ids.length) { focusRow = ids[0]; renderResults(); focusGridRow(); } }
  });
  $("btnRetry").onclick = retryLastSearch;
  $("btnMetadataOnly").onclick = () => checkMetadataPdfs(true);
  $("btnGapReport").onclick = showGapReport;
  $("btnToggleConfig").onclick = () => toggleConfig();
  $("exportScope").onchange = () => renderResults();
  $("compactRows").onchange = () => { applyDensity(); scheduleSave(); };
  $("btnColumns").onclick = (e) => {
    e.stopPropagation();
    toggleMenu("columnsMenu", "btnColumns", !$("columnsMenu").classList.contains("open"));
  };
  for (const box of document.querySelectorAll("#columnsMenu input[data-col]")) {
    box.onchange = () => {
      const key = box.dataset.col;
      if (box.checked) hiddenColumns.delete(key); else hiddenColumns.add(key);
      renderResults();
      scheduleSave();
    };
  }
  $("btnDetailOpenLink").onclick = () => { const a = storedArticles[detailIndex]; if (a) openRecordLink(a); };
  $("btnDetailAbstract").onclick = () => { if (detailIndex !== -1) openAbstract(detailIndex); };
  $("btnCloseAbstract").onclick = () => $("abstractDialog").close();
  $("btnDetailOpenPdf").onclick = () => { const a = storedArticles[detailIndex]; if (a) revealRecordPdf(a); };
  $("btnDetailCopyDoi").onclick = () => { const a = storedArticles[detailIndex]; if (a) copyText(a.doi || ""); };
  $("btnDetailCopyTitle").onclick = () => { const a = storedArticles[detailIndex]; if (a) copyText(a.title || ""); };
  $("tableBody").ondblclick = onTableDblClick;
  $("tableBody").oncontextmenu = onTableContextMenu;
  $("btnReveal").onclick = () => {
    if (lastExportFolder) invoke("reveal_path", { path: lastExportFolder }).catch(() => {});
  };
  $("btnErrorClose").onclick = () => $("errorDialog").close();
  $("btnFileMenu").onclick = (e) => {
    e.stopPropagation();
    toggleMenu("fileMenu", "btnFileMenu", !$("fileMenu").classList.contains("open"));
  };
  $("menuExportCitations").onclick = () => {
    toggleMenu("fileMenu", "btnFileMenu", false);
    openCitationsDialog();
  };
  $("menuOpenFolder").onclick = () => {
    toggleMenu("fileMenu", "btnFileMenu", false);
    if (lastExportFolder) invoke("reveal_path", { path: lastExportFolder }).catch(() => {});
  };
  for (const th of document.querySelectorAll("th.sortable")) {
    th.onclick = () => onHeaderClick(th.dataset.key);
  }
  $("resultsSearch").oninput = () => {
    resultsQuery = $("resultsSearch").value.trim();
    scheduleResultsRender();
  };
  document.addEventListener("click", closeMenus);
  document.addEventListener("keydown", onKeyDown);
  for (const id of ["query", "pages", "folder", "rankingPath", "rankingYear", "yearFrom", "yearTo"]) {
    $(id).addEventListener("input", scheduleSave);
  }
  $("rankingEnabled").addEventListener("change", scheduleSave);
  $("yearPreset").onchange = () => {
    onYearPresetChanged();
    renderResults();
    scheduleSave();
  };
  window.addEventListener("beforeunload", () => {
    saveSettingsNow();
  });
  setIdleState();
  loadSettings();
}

// ---- keyboard ----
function onKeyDown(e) {
  const active = document.activeElement;
  const inField = active && active.closest("input, textarea, select");
  if (e.key === "Escape") {
    if (closeMenus()) {
      e.preventDefault();
      return;
    }
    if (running) {
      cancelOperation();
      return;
    }
    // Nothing else to dismiss: Escape drops the selection.
    if (selection.size > 0) {
      selection.clear();
      renderResults();
      e.preventDefault();
    }
    return;
  }
  if (running) return;
  if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "e") {
    e.preventDefault();
    openCitationsDialog();
    return;
  }
  if (e.ctrlKey && !e.altKey && e.shiftKey && e.key.toLowerCase() === "a") {
    e.preventDefault();
    selection.clear();
    renderResults();
    return;
  }
  if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "a" && !inField) {
    e.preventDefault();
    for (const [i] of visibleArticles()) selection.add(i);
    renderResults();
    return;
  }
  // Arrow recall in an empty query field.
  if (active === $("query") && !$("query").value.trim() && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
    if (queryHistory.length > 0) {
      e.preventDefault();
      if (e.key === "ArrowUp") historyCursor = Math.min(historyCursor + 1, queryHistory.length - 1);
      else historyCursor = Math.max(historyCursor - 1, 0);
      $("query").value = queryHistory[historyCursor] || "";
    }
    return;
  }
  if (e.key === "Enter" && !e.shiftKey) {
    // Deliberately narrower than the Python global binding: years and the
    // query start a search, but typing in pages/folder/ranking fields does not.
    if (active === $("query") || active === $("yearFrom") || active === $("yearTo")) {
      startSearch();
    }
  }
}

// ---- keyboard navigation over the results grid ----
function visibleIndices() {
  return visibleArticles().map(([i]) => i);
}

function moveFocus(delta) {
  const ids = visibleIndices();
  if (ids.length === 0) return;
  const at = ids.indexOf(focusRow);
  const next = at === -1
    ? (delta > 0 ? 0 : ids.length - 1)
    : Math.min(Math.max(at + delta, 0), ids.length - 1);
  focusRow = ids[next];
  renderResults();
  focusGridRow();
}

function focusGridRow() {
  const tr = document.querySelector(`#tableBody tr[data-iid="article-${focusRow}"]`);
  if (tr) tr.focus();
}

function onTableKeyDown(e) {
  const ids = visibleIndices();
  if (ids.length === 0) return;
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      if (e.shiftKey) extendSelection(1);
      else { moveFocus(1); anchorRow = focusRow; }
      break;
    case "ArrowUp":
      e.preventDefault();
      if (e.shiftKey) extendSelection(-1);
      else { moveFocus(-1); anchorRow = focusRow; }
      break;
    case "Home":
      e.preventDefault();
      focusRow = ids[0]; anchorRow = focusRow; renderResults(); focusGridRow();
      break;
    case "End":
      e.preventDefault();
      focusRow = ids[ids.length - 1]; anchorRow = focusRow; renderResults(); focusGridRow();
      break;
    case " ":
    case "Spacebar":
      if (focusRow === -1) return;
      e.preventDefault();
      if (selection.has(focusRow)) selection.delete(focusRow);
      else selection.add(focusRow);
      renderResults();
      focusGridRow();
      break;
    case "Enter":
      if (focusRow !== -1) {
        e.preventDefault();
        openDetailFor(focusRow);
      }
      break;
    default:
      break;
  }
}

function extendSelection(delta) {
  const ids = visibleIndices();
  if (anchorRow === -1) anchorRow = focusRow === -1 ? ids[0] : focusRow;
  const from = ids.indexOf(anchorRow);
  const to = Math.min(Math.max((focusRow === -1 ? from : ids.indexOf(focusRow)) + delta, 0), ids.length - 1);
  if (from === -1) return;
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  for (let k = lo; k <= hi; k++) selection.add(ids[k]);
  focusRow = ids[to];
  renderResults();
  focusGridRow();
}

// ---- menus ----
function toggleMenu(menuId, buttonId, open) {
  const menu = $(menuId);
  const btn = $(buttonId);
  menu.classList.toggle("open", open);
  btn.classList.toggle("open", open);
}

function closeMenus() {
  let closed = false;
  for (const [menuId, buttonId] of [["fileMenu", "btnFileMenu"]]) {
    if ($(menuId) && $(menuId).classList.contains("open")) {
      toggleMenu(menuId, buttonId, false);
      closed = true;
    }
  }
  const ctx = document.querySelector(".ctxmenu");
  if (ctx) {
    ctx.remove();
    ctxTarget = null;
    closed = true;
  }
  return closed;
}

// ---- sortable columns ----
function onHeaderClick(key) {
  if (sortKey === key) {
    sortDir = sortDir === 1 ? -1 : 1;
  } else {
    sortKey = key;
    sortDir = 1;
  }
  renderResults();
  scheduleSave();
}

// Journal indexing labels. Mirrors gss_core::catalog::indexing_label; the UI
// cannot call Rust, so the rule is duplicated here and covered by a unit test
// on the Rust side.
function indexingLabels(a) {
  const labels = [];
  const sjr = String(a.sjrQuartile ?? a.sjr_quartile ?? "").trim();
  if (sjr) labels.push(sjr.toUpperCase() === "SJR" ? "SJR" : `SJR ${sjr}`);
  const sinta = String(a.sintaRank ?? a.sinta_rank ?? "").trim();
  if (sinta) labels.push(sinta.toUpperCase() === "SINTA" ? "SINTA" : `SINTA ${sinta}`);
  const sources = [a.sjrSourceUrl ?? a.sjr_source_url, a.sintaSourceUrl ?? a.sinta_source_url, a.pdfSourceUrl ?? a.pdf_source_url]
    .map((u) => String(u ?? "").toLowerCase());
  if (sources.some((u) => u.includes("scopus"))) labels.push("SCOPUS");
  if (sources.some((u) => u.includes("doaj"))) labels.push("DOAJ");
  return labels;
}

function indexingLabel(a) {
  return indexingLabels(a).join(", ");
}

function sortValue(a, key) {
  switch (key) {
    case "title":
      return (a.title || "").toLocaleLowerCase();
    case "authors":
      return (a.authors || "").toLocaleLowerCase();
    case "index":
      return indexingLabel(a).toLocaleLowerCase();
    case "rank":
      return [a.sjrQuartile ?? a.sjr_quartile, a.sintaRank ?? a.sinta_rank]
        .filter(Boolean).join(" ").toLocaleLowerCase();
    case "pdf": {
      // Group sensibly: available → unknown → unavailable.
      const status = a.pdfStatus ?? a.pdf_status;
      return status === "available" ? 0 : status === "unavailable" ? 2 : 1;
    }
    case "link":
      return (a.link || "").toLocaleLowerCase();
    case "cited": {
      const cited = a.citedBy ?? a.cited_by;
      return cited == null ? -1 : Number(cited);
    }
    case "oa":
      return (a.oaStatus ?? a.oa_status ?? "").toLocaleLowerCase();
    default:
      return "";
  }
}

function sortRows(rows) {
  if (!sortKey) return rows;
  // Copy: the filter result is rebuilt per render, but sorting in place would
  // still alias the caller's array.
  const sorted = rows.slice();
  sorted.sort((x, y) => {
    const a = sortValue(x[1], sortKey);
    const b = sortValue(y[1], sortKey);
    let cmp;
    if (typeof a === "number" && typeof b === "number") cmp = a - b;
    else cmp = String(a).localeCompare(String(b));
    // Stable tie-break on the stored index keeps equal rows in retrieval order.
    return cmp !== 0 ? cmp * sortDir : x[0] - y[0];
  });
  return sorted;
}

function onYearPresetChanged() {
  const idx = $("yearPreset").selectedIndex;
  const custom = idx === 4;
  $("yearFrom").disabled = !custom || running;
  $("yearTo").disabled = !custom || running;
  updateAppliedYear();
}

function currentYearBounds() {
  const idx = $("yearPreset").selectedIndex;
  if (idx === 4) {
    const from = parseInt($("yearFrom").value, 10);
    const to = parseInt($("yearTo").value, 10);
    return { from: Number.isInteger(from) ? from : null, to: Number.isInteger(to) ? to : null };
  }
  const opts = yearFilterOptions(currentYear);
  const o = opts[idx];
  return { from: o.from, to: o.to };
}

function updateAppliedYear() {
  const { from, to } = currentYearBounds();
  $("appliedYear").textContent = yearRangeLabel(from, to);
}

// ---- state transitions ----
function setRunning(operation) {
  running = true;
  currentOperation = operation;
  for (const id of ["btnSearch", "btnCheck", "btnDownload", "btnDetails", "btnImport", "btnBrowse", "btnExportExcel", "btnExportCsv", "btnExportCitations", "menuExportCitations", "menuOpenFolder"]) {
    $(id).disabled = true;
  }
  closeMenus();
  $("btnCancel").disabled = false;
  $("query").disabled = true;
  $("pages").disabled = true;
  $("folder").disabled = true;
  $("rankingPath").disabled = true;
  $("rankingYear").disabled = true;
  $("rankingEnabled").disabled = true;
  $("pdfFilter").disabled = true;
  $("yearPreset").disabled = true;
  $("yearFrom").disabled = true;
  $("yearTo").disabled = true;
}

function setIdleState() {
  // Controls only. Rendering is the caller's job: this function must never
  // call renderResults() (that edge is what made v3.0.0 recurse to a stack
  // overflow inside init() and left every control dead).
  running = false;
  currentOperation = null;
  const hasArticles = storedArticles.length > 0;
  const hasVisible = visibleArticles().length > 0;
  $("btnSearch").disabled = false;
  $("btnCancel").disabled = true;
  $("btnCheck").disabled = !hasArticles;
  $("btnMetadataOnly").disabled = !hasArticles;
  $("btnDownload").disabled = !hasArticles;
  $("btnDetails").disabled = !hasArticles;
  $("btnImport").disabled = false;
  $("btnBrowse").disabled = false;
  $("btnExportExcel").disabled = !hasVisible;
  $("btnExportCsv").disabled = !hasVisible;
  $("btnExportCitations").disabled = !hasVisible;
  $("menuExportCitations").disabled = !hasVisible;
  $("menuOpenFolder").disabled = !lastExportFolder;
  $("btnReveal").hidden = !lastExportFolder;
  syncColumnCheckboxes();
  $("query").disabled = false;
  $("pages").disabled = false;
  $("folder").disabled = false;
  $("rankingPath").disabled = false;
  $("rankingYear").disabled = false;
  $("rankingEnabled").disabled = false;
  $("pdfFilter").disabled = false;
  $("yearPreset").disabled = false;
  const customYear = $("yearPreset").selectedIndex === 4;
  $("yearFrom").disabled = !customYear;
  $("yearTo").disabled = !customYear;
  updateAppliedYear();
}

// ---- status helpers ----
function setStatus(heading, detail, tone) {
  $("statusHeading").textContent = heading;
  $("statusDetail").textContent = detail;
  $("statusPanel").className = `panel status ${tone || "ready"}`;
}
function setProgress(current, total) {
  const pct = total > 0 ? Math.min(100, Math.max(0, Math.round((current / total) * 100))) : 0;
  $("progress").value = pct;
}

// ---- search ----
async function startSearch() {
  if (running) return;
  const pagesText = $("pages").value.trim();
  const pagesValue = /^\d+$/.test(pagesText) ? Number(pagesText) : NaN;
  if (!Number.isInteger(pagesValue)) {
    setStatus("Check search setup", "Page count must be a whole number.", "error");
    return;
  }
  if (pagesValue <= 0) {
    setStatus("Check search setup", "Page count must be greater than zero.", "error");
    return;
  }
  const { from, to } = currentYearBounds();
  const request = {
    query: $("query").value,
    pages: pagesValue,
    rankingEnabled: $("rankingEnabled").checked,
    outputFolder: $("folder").value,
    yearFrom: from,
    yearTo: to,
  };
  lastRequest = { ...request };
  try {
    await invoke("search_scholar", request);
  } catch (err) {
    // Validation errors: inline status, no dialog (matches Python).
    setStatus("Check search setup", errMessage(err), "error");
    return;
  }
  storedArticles = [];
  selection.clear();
  renderResults();
  setProgress(0, 100);
  $("summary").textContent = "Searching...";
  setStatus(OPERATION_HEADINGS.search, "Results will appear here as soon as the search completes.", "running");
  setRunning("search");
}

function cancelOperation() {
  if (!running) return;
  $("btnCancel").disabled = true;
  const op = currentOperation;
  const heading =
    op === "import" ? "Cancelling import" :
    op === "download" ? "Cancelling download" :
    op === "check" ? "Cancelling check" : "Cancelling search";
  const detail = op === "import" ? "The loaded ranking records are kept." : "Finishing the current safe stop point...";
  setStatus(heading, detail, "warning");
  invoke("cancel_operation");
}

// ---- check / download / import ----
async function checkMetadataPdfs(metadataOnly = false) {
  if (running || storedArticles.length === 0) return;
  const detail = metadataOnly
    ? "Refreshing bibliographic metadata only; public-PDF links are left as they are."
    : "Looking up verified bibliographic metadata, then validating public PDF links. Records stay usable if a provider fails.";
  setStatus(OPERATION_HEADINGS.check, detail, "running");
  setRunning("check");
  try {
    await invoke("check_metadata_pdfs", { metadataOnly });
  } catch (err) {
    finishOperation();
    if (errCode(err) === "NOTHING_TO_CHECK") {
      setStatus("Nothing to check", "Run a search first, then check metadata and public PDFs.", "warning");
    } else {
      taskError("check", errMessage(err));
    }
  }
}

async function importRanking() {
  if (running) return;
  const path = $("rankingPath").value.trim() || (await dialog("open", "Choose a journal ranking CSV or XLSX file"));
  if (!path) {
    setStatus("Import cancelled", "No ranking file was chosen.", "warning");
    return;
  }
  $("rankingPath").value = path;
  const yearText = $("rankingYear").value.trim();
  let year = null;
  if (yearText) {
    // tkinter_app.py L993-1002: two distinct messages, not one combined check.
    year = /^[+-]?\d+$/.test(yearText) ? Number(yearText) : NaN;
    if (!Number.isInteger(year)) {
      setStatus("Check ranking year", "Ranking year must be a whole number, or empty when the file supplies it.", "error");
      return;
    }
    if (year < 1500 || year > 2100) {
      setStatus("Check ranking year", "Ranking year must be between 1500 and 2100.", "error");
      return;
    }
  }
  setStatus(OPERATION_HEADINGS.import, "Validating the file in the background. A rejected file leaves the loaded records untouched.", "running");
  setRunning("import");
  try {
    await invoke("import_ranking", { path, defaultYear: year });
  } catch (err) {
    finishOperation();
    if (errCode(err) === "NO_FILE_FOUND") {
      setStatus("Check ranking file", errMessage(err), "error");
    } else {
      taskError("import", errMessage(err));
    }
  }
}

async function downloadSelected() {
  if (running) return;
  if (selection.size === 0) {
    setStatus("No papers selected", "Select one or more rows (Ctrl/Shift click), then download the accessible public PDFs.", "warning");
    return;
  }
  const folder = $("folder").value.trim() || (await dialog("pickFolder", "Choose a folder for the downloaded PDFs"));
  if (!folder) {
    setStatus("Download cancelled", "No destination folder was chosen.", "warning");
    return;
  }
  setStatus(OPERATION_HEADINGS.download, `Checking and saving public PDFs for ${selection.size} selected record(s).`, "running");
  setRunning("download");
  try {
    await invoke("download_pdfs", { indices: [...selection], folder });
  } catch (err) {
    finishOperation();
    taskError("download", errMessage(err));
  }
}

// ---- citations ----
function openCitationsDialog() {
  if (running) return;
  if (storedArticles.length === 0) {
    setStatus("No results to export.", "Run a search before exporting citations.", "warning");
    return;
  }
  $("citationsDialog").showModal();
}

async function startCitationExport() {
  if (running) return;
  const scope = document.querySelector('input[name="citScope"]:checked').value;
  const fmt = document.querySelector('input[name="citFormat"]:checked').value;
  let records;
  if (scope === "selected") {
    if (selection.size === 0) {
      setStatus("No papers selected", "Select one or more rows first, or export the visible or all collected records.", "warning");
      return;
    }
    records = [...selection].map((i) => storedArticles[i]).filter(Boolean);
  } else if (scope === "visible") {
    records = visibleArticles().map(([, a]) => a);
  } else {
    records = storedArticles.slice();
  }
  if (records.length === 0) {
    setStatus("No results to export.", "Run a search before exporting citations.", "warning");
    return;
  }
  const folder = currentRequest?.outputFolder || $("folder").value.trim();
  const filename = fmt === "bibtex" ? DEFAULT_EXPORT_NAMES.bibtex : DEFAULT_EXPORT_NAMES.ris;
  const path = joinPath(folder, filename);
  $("citationsDialog").close();
  setStatus(OPERATION_HEADINGS.citations, "Resolving citations against Crossref, then writing the file. Unresolved records keep their known fields and a review note.", "running");
  setRunning("citations");
  try {
    await invoke("export_citations", { articles: records, format: fmt, path });
  } catch (err) {
    finishOperation();
    taskError("citations", errMessage(err));
  }
}

// ---- spreadsheet exports (synchronous on the UI thread) ----
async function exportSpreadsheet(kind) {
  // Scope mirrors the citations dialog: visible (filtered) rows, or the rows the
  // user selected — the button labels now say which is which.
  const scope = $("exportScope").value || "visible";
  const visible = scope === "selected"
    ? [...selection].map((i) => storedArticles[i]).filter(Boolean)
    : visibleArticles().map(([, a]) => a);
  if (visible.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const folder = currentRequest?.outputFolder || $("folder").value.trim();
  const filename = kind === "excel" ? DEFAULT_EXPORT_NAMES.excel : DEFAULT_EXPORT_NAMES.csv;
  const path = joinPath(folder, filename);
  const cmdName = kind === "excel" ? "export_excel" : "export_csv";
  try {
    const message = await invoke(cmdName, { articles: visible, path });
    const total = storedArticles.length;
    const detail = visible.length === total
      ? `Exported ${visible.length} result(s) to ${path}.`
      : `Exported ${visible.length} filtered result(s) of ${total} stored records to ${path}.`;
    setStatus("Ready", detail, "ready");
    rememberExportFolder(path);
  } catch (err) {
    const msg = errMessage(err).replace("Export failed: ", "");
    setStatus("Ready", `Export failed: ${msg}`, "error");
    showError("Export failed", msg);
  }
}

// ---- table rendering (iid = article-{storedIndex}) ----
function filteredArticles() {
  const needle = resultsQuery.toLocaleLowerCase();
  const rows = storedArticles
    .map((a, i) => [i, a])
    .filter(([, a]) => {
      if (pdfFilter !== "All") {
        const want = pdfFilter === "Public PDF available" ? "available"
          : pdfFilter === "No public PDF found" ? "unavailable" : "unknown";
        const status = a.pdfStatus ?? a.pdf_status;
        if (status !== want) return false;
      }
      if (!needle) return true;
      const haystack = [a.title, a.authors, a.journal]
        .map((v) => String(v ?? "").toLocaleLowerCase())
        .join("\n");
      return haystack.includes(needle);
    });
  return sortRows(rows);
}
function visibleArticles() {
  return filteredArticles();
}

function renderResults() {
  pdfFilter = $("pdfFilter").value || "All";
  const rows = filteredArticles();
  const body = $("tableBody");
  const previousSelection = new Set(selection);
  body.innerHTML = "";
  for (const [index, a] of rows) {
    const tr = document.createElement("tr");
    tr.dataset.iid = `article-${index}`;
    tr.tabIndex = -1;
    tr.setAttribute("role", "row");
    tr.setAttribute("aria-selected", selection.has(index) ? "true" : "false");
    if (previousSelection.has(index)) tr.classList.add("selected");
    if (index === focusRow) tr.classList.add("focused");
    const title = a.title ?? "";
    const authors = a.authors ?? "";
    const rank = [a.sjrQuartile ?? a.sjr_quartile, a.sintaRank ?? a.sinta_rank]
      .filter(Boolean).join(" / ");
    const status = a.pdfStatus ?? a.pdf_status;
    const localPath = a.pdfLocalPath ?? a.local_path ?? a.pdf_local_path;
    const pdfUrl = a.pdfUrl ?? a.pdf_url ?? "";
    const link = a.link ?? "";
    const cells = [
      ["title", title, null],
      ["authors", authors, null],
      ["index", indexingLabel(a), null],
      ["rank", rank, null],
      ["pdf", status === "available" ? (localPath ? "Downloaded" : "Public PDF")
        : status === "unavailable" ? "No public PDF" : "Unknown",
        localPath ? { action: "reveal-pdf", value: localPath, label: "Downloaded" }
          : pdfUrl ? { action: "open-pdf", value: pdfUrl, label: "Public PDF" } : null],
      ["link", link ? "Open link" : "No link",
        link ? { action: "open-link", value: link, label: "Open link" } : null],
      ["cited", (a.citedBy ?? a.cited_by) == null ? "" : String(a.citedBy ?? a.cited_by), null],
      ["oa", (a.oaStatus ?? a.oa_status ?? "").trim(), null],
    ];
    for (const [key, text, action] of cells) {
      if (hiddenColumns.has(key)) continue;
      const td = document.createElement("td");
      td.dataset.col = key;
      td.title = text; // full value survives truncation
      // A retracted or corrected record must be visible without opening it.
      const notice = (a.retraction ?? "").trim();
      if (notice && key === "title") {
        const badge = document.createElement("span");
        badge.className = "badge retracted";
        badge.textContent = notice === "retraction" ? "RETRACTED" : notice.toUpperCase();
        badge.title = (a.retractionDoi ?? a.retraction_doi)
          ? `Notice DOI: ${a.retractionDoi ?? a.retraction_doi}`
          : "This record carries a correction notice";
        td.appendChild(badge);
        td.appendChild(document.createTextNode(" "));
      }
      if (action) {
        const btn = document.createElement("button");
        btn.className = "linkbtn";
        btn.textContent = action.label;
        btn.dataset.action = action.action;
        btn.dataset.value = action.value;
        td.textContent = "";
        td.appendChild(btn);
      } else {
        td.textContent = text;
      }
      tr.appendChild(td);
    }
    body.appendChild(tr);
  }
  const hasAny = storedArticles.length > 0;
  $("tableWrap").classList.toggle("hidden", rows.length === 0);
  $("emptyState").classList.toggle("hidden", rows.length > 0);
  if (rows.length === 0) {
    if (hasAny) {
      $("emptyTitle").textContent = "No papers match this filter";
      $("emptyDetail").textContent = resultsQuery
        ? "Clear the results filter or the PDF filter to see the stored records. Filtering never removes stored results."
        : "Change the PDF filter to see the stored records. Filtering never removes stored results.";
    } else {
      $("emptyTitle").textContent = "No results yet";
      $("emptyDetail").textContent = "Run a search to review Google Scholar results here.";
    }
  }
  updateSortIndicators();
  applyDensity();
  updateYearHistogram();
  refreshSummary();
  // One-way edge: render → idle. setIdleState() never renders.
  setIdleState();
}

function updateSortIndicators() {
  for (const th of document.querySelectorAll("th.sortable")) {
    const base = th.textContent.replace(/\s*[\u25b2\u25bc]$/, "");
    const active = th.dataset.key === sortKey;
    th.classList.toggle("sorted", active);
    th.textContent = active ? `${base} ${sortDir === 1 ? "\u25b2" : "\u25bc"}` : base;
    th.setAttribute("aria-sort", active ? (sortDir === 1 ? "ascending" : "descending") : "none");
  }
  syncColumnCheckboxes();
}

// Publication-year distribution: "1998-2024, 60% since 2020".
function updateYearHistogram() {
  const years = storedArticles
    .map((a) => a.publicationYear ?? a.publication_year)
    .filter((y) => Number.isInteger(y));
  const el = $("yearHistogram");
  if (!el) return;
  if (years.length === 0) {
    el.textContent = "";
    return;
  }
  const min = Math.min(...years);
  const max = Math.max(...years);
  const recentCutoff = max - 5;
  const recent = years.filter((y) => y >= recentCutoff).length;
  const pct = Math.round((recent / years.length) * 100);
  el.textContent = `${min}\u2013${max}, ${pct}% since ${recentCutoff}`;
}

function refreshSummary() {
  const rows = filteredArticles();
  const total = storedArticles.length;
  const filtering = pdfFilter !== "All" || resultsQuery !== "";
  let text;
  if (total === 0 || !filtering) {
    text = summaryBase;
  } else if (pdfFilter !== "All") {
    text = `${summaryBase} | filter '${pdfFilter}': ${rows.length} of ${total} shown`;
  } else {
    text = `${summaryBase} | matching '${resultsQuery}': ${rows.length} of ${total} shown`;
  }
  // Additive segment: the oracle-mirrored text above keeps its exact wording.
  if (selection.size > 0) text += ` | ${selection.size} selected`;
  $("summary").textContent = text;
}

function onTableClick(e) {
  // Cell actions run first and never fall through to row selection.
  const action = e.target.closest("[data-action]");
  if (action) {
    e.preventDefault();
    e.stopPropagation();
    const value = action.dataset.value || "";
    if (action.dataset.action === "reveal-pdf") invoke("reveal_path", { path: value }).catch(() => {});
    else if (action.dataset.action === "open-pdf" || action.dataset.action === "open-link") openUrl(value);
    return;
  }
  const tr = e.target.closest("tr[data-iid]");
  if (!tr) return;
  const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
  if (e.ctrlKey || e.metaKey || e.shiftKey) {
    if (selection.has(index)) selection.delete(index);
    else selection.add(index);
  } else {
    selection.clear();
    selection.add(index);
  }
  document.querySelectorAll("#tableBody tr").forEach((row) => {
    const i = parseInt(row.dataset.iid.replace("article-", ""), 10);
    row.classList.toggle("selected", selection.has(i));
  });
  setIdleState(); // selection changed → export/detail buttons follow it
}

function onTableDblClick(e) {
  const tr = e.target.closest("tr[data-iid]");
  if (!tr) return;
  const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
  const a = storedArticles[index];
  if (a && (a.link || "")) openUrl(a.link);
}

function openUrl(url) {
  if (!url) return;
  invoke("open_url", { url }).catch(() => {});
}

// ---- row context menu ----
function onTableContextMenu(e) {
  const tr = e.target.closest("tr[data-iid]");
  if (!tr) return;
  e.preventDefault();
  closeMenus();
  const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
  const a = storedArticles[index];
  if (!a) return;
  ctxTarget = index;
  const link = a.link || "";
  const doi = a.doi || "";
  const localPath = a.pdfLocalPath ?? a.pdf_local_path ?? "";
  const menu = document.createElement("div");
  menu.className = "ctxmenu";
  menu.style.left = `${e.clientX}px`;
  menu.style.top = `${e.clientY}px`;
  const items = [
    ["Open link", !link, () => openRecordLink(a)],
    ["Abstract", false, () => openAbstract(index)],
    ["Reveal PDF in Explorer", !localPath, () => revealRecordPdf(a)],
    ["Copy title", !a.title, () => copyText(a.title || "")],
    ["Copy DOI", !doi, () => copyText(doi)],
    ["Details", false, () => openDetailFor(index)],
  ];
  for (const [label, disabled, action] of items) {
    const btn = document.createElement("button");
    btn.textContent = label;
    btn.disabled = disabled;
    btn.onclick = () => {
      closeMenus();
      action();
    };
    menu.appendChild(btn);
  }
  document.body.appendChild(menu);
}

function copyText(text) {
  if (!text) return;
  if (navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text).catch(() => {});
  }
}

async function appendRankTrend(index) {
  const a = storedArticles[index];
  if (!a) return;
  const issns = a.issns ?? [];
  const journal = a.journal ?? "";
  if (!journal && issns.length === 0) return;
  let history = [];
  try {
    history = await invoke("journal_rank_history", { issns, journal });
  } catch {
    return;
  }
  if (!Array.isArray(history) || history.length < 2) return;
  // Only render if the same record is still open.
  if (detailIndex !== index || !$("detailDialog").open) return;
  const trend = history.map(([year, kind, rank]) => `${kind.toUpperCase()} ${rank}${year ? ` (${year})` : ""}`).join(" \u2192 ");
  const text = $("detailText").textContent;
  const marker = "\n\nPublic PDF:";
  $("detailText").textContent = text.includes(marker)
    ? text.replace(marker, `\nJournal trend: ${trend}${marker}`)
    : `${text}\nJournal trend: ${trend}`;
}

function openDetailFor(index) {
  const a = storedArticles[index];
  if (!a) return;
  detailIndex = index;
  $("detailText").textContent = articleDetailText(a);
  $("btnDetailOpenLink").disabled = !(a.link || "");
  $("btnDetailOpenPdf").disabled = !(a.pdfLocalPath ?? a.pdf_local_path ?? a.pdfUrl ?? a.pdf_url);
  $("btnDetailCopyDoi").disabled = !(a.doi || "");
  $("btnDetailCopyTitle").disabled = !(a.title || "");
  $("detailDialog").showModal();
  appendRankTrend(index);
}

async function openAbstract(index) {
  const a = storedArticles[index];
  if (!a) return;
  $("abstractTitle").textContent = a.title || "Abstract";
  $("abstractText").textContent = "Fetching abstract…";
  $("abstractDialog").showModal();
  let text = null;
  try { text = await invoke("fetch_abstract", { article: a }); } catch { text = null; }
  if (text) {
    $("abstractText").textContent = text;
  } else {
    $("abstractText").textContent = "No abstract is available from Crossref or OpenAlex for this record. Open the link to read the paper at its source.";
  }
}

function openRecordLink(a) {
  if (a && a.link) openUrl(a.link);
}

function revealRecordPdf(a) {
  if (!a) return;
  const localPath = a.pdfLocalPath ?? a.pdf_local_path ?? "";
  if (localPath) { invoke("reveal_path", { path: localPath }).catch(() => {}); return; }
  const url = a.pdfUrl ?? a.pdf_url ?? "";
  if (url) openUrl(url);
}

// ---- details dialog ----
function openDetail() {
  if (selection.size === 0) {
    setStatus("No paper selected", "Select a row to see its journal, ranking and public-PDF details.", "warning");
    return;
  }
  openDetailFor([...selection][0]);
}

function articleDetailText(a) {
  // Verbatim from _open_detail_window's lines list (tkinter_app.py L315-339).
  const none = "unknown";
  const lines = [a.title || "(untitled)"];
  lines.push(`Authors: ${a.authors || none}`);
  lines.push(`Link: ${a.link || "none"}`);
  lines.push(`DOI: ${a.doi || none}`);
  lines.push(`Journal: ${a.journal || none}`);
  lines.push(`ISSN: ${(a.issns ?? []).length ? a.issns.join(", ") : none}`);
  lines.push(`Publication year: ${a.publicationYear ?? a.publication_year ?? none}`);
  // v3.4.0 metadata (details dialog only; the table stays narrow).
  const venue = [(a.volume ?? ""), (a.issue ?? "")].filter(Boolean).join("(") +
    ((a.volume ?? "") && (a.issue ?? "") ? ")" : "");
  if (venue) lines.push(`Volume/issue: ${venue}`);
  if (a.pages) lines.push(`Pages: ${a.pages}`);
  if (a.publisher) lines.push(`Publisher: ${a.publisher}`);
  if (a.citedBy ?? a.cited_by ?? null) lines.push(`Cited by: ${a.citedBy ?? a.cited_by}`);
  if (a.oaStatus ?? a.oa_status) lines.push(`Open access: ${a.oaStatus ?? a.oa_status}`);
  if (a.license) lines.push(`License: ${a.license}`);
  if (a.pmid) lines.push(`PMID: ${a.pmid}`);
  if (a.pmcid) lines.push(`PMCID: ${a.pmcid}`);
  if ((a.orcids ?? []).length) lines.push(`ORCID: ${a.orcids.join(", ")}`);
  if ((a.funders ?? []).length) lines.push(`Funding: ${a.funders.join("; ")}`);
  if (a.retraction) {
    const notice = a.retractionDoi ?? a.retraction_doi;
    lines.push(`Notice: ${a.retraction}${notice ? ` (${notice})` : ""}`);
  }
  lines.push("");
  const sjrYear = a.sjrYear ?? a.sjr_year;
  const sjrBasis = (a.sjrMatchedBy ?? a.sjr_matched_by ?? "").trim();
  const sjrNote = sjrBasis ? `  [matched by ${sjrBasis}]` : "";
  lines.push(`SJR quartile: ${(a.sjrQuartile ?? a.sjr_quartile) || none}   (ranking year: ${sjrYear ?? none})${sjrNote}`);
  lines.push(`SJR category: ${(a.sjrCategory ?? a.sjr_category) || none}`);
  lines.push(`SJR source: ${(a.sjrSourceUrl ?? a.sjr_source_url) || "no imported record"}`);
  const sintaYear = a.sintaYear ?? a.sinta_year;
  const sintaBasis = (a.sintaMatchedBy ?? a.sinta_matched_by ?? "").trim();
  const sintaNote = sintaBasis ? `  [matched by ${sintaBasis}]` : "";
  lines.push(`SINTA rank: ${(a.sintaRank ?? a.sinta_rank) || none}   (ranking year: ${sintaYear ?? none})${sintaNote}`);
  lines.push(`SINTA source: ${(a.sintaSourceUrl ?? a.sinta_source_url) || "no imported record"}`);
  lines.push("");
  // Python prints the raw stored status; no relabelling.
  lines.push(`PDF status: ${a.pdfStatus ?? a.pdf_status}`);
  lines.push(`PDF detail: ${(a.pdfDetail ?? a.pdf_detail) || "not checked"}`);
  lines.push(`PDF source: ${(a.pdfSourceUrl ?? a.pdf_source_url) || "none"}`);
  lines.push(`PDF local path: ${(a.pdfLocalPath ?? a.pdf_local_path) || "not downloaded"}`);
  lines.push("");
  lines.push(DETAIL_DISCLAIMER);
  return lines.join("\n");
}

// ---- worker events ----
async function onWorkerMessage(msg) {
  if (msg.kind === "progress") {
    setProgress(msg.current, msg.total);
    if (msg.operation === "search") {
      setStatus(OPERATION_HEADINGS.search, `${msg.phase.charAt(0).toUpperCase()}${msg.phase.slice(1)} page ${msg.current} of ${msg.total}...`, "running");
    } else {
      setStatus(OPERATION_HEADINGS[msg.operation] || "Working", `${msg.phase} (${msg.current}/${msg.total})`, "running");
    }
    return;
  }
  if (msg.kind === "result") {
    finishOperation();
    const r = msg.result;
    const status = r.status ?? r.Status;
    if (!r || !status) {
      storedArticles = [];
      renderResults();
      setStatus("Search failed", "The application did not receive a valid result.", "error");
      return;
    }
    storedArticles = r.articles ?? [];
    currentRequest = r.request ?? currentRequest;
    const requested = r.requestedPages ?? r.requested_pages ?? 0;
    const successful = r.successfulPages ?? r.successful_pages ?? 0;
    const duplicates = r.duplicatesRemoved ?? r.duplicates_removed ?? 0;
    const invalid = r.invalidArticlesRemoved ?? r.invalid_articles_removed ?? 0;
    // result_summary() L249-261: singular/plural, not "(s)".
    const resultWord = storedArticles.length === 1 ? "result" : "results";
    // Python appends the plural "s" to "removed" (L255/L259) — reproduced verbatim.
    const byDoi = r.duplicatesByDoi ?? r.duplicates_by_doi ?? 0;
    summaryBase = `${storedArticles.length} ${resultWord} found | ${successful}/${requested} pages completed` +
      (duplicates ? ` | ${duplicates} duplicate removed${duplicates === 1 ? "" : "s"}` : "") +
      (byDoi ? ` (${byDoi} matched by DOI)` : "") +
      (invalid ? ` | ${invalid} invalid record removed${invalid === 1 ? "" : "s"}` : "");
    const { from, to } = currentYearBounds();
    $("appliedYear").textContent = yearRangeLabel(from, to);
    setProgress(successful, requested);
    let message = r.message || STATUS_DEFAULTS[status] || "";
    // status_message() L203-206: the hint rides along with the message.
    if (status === "SUCCESS") message += CHECK_METADATA_HINT;
    setStatus(STATUS_HEADINGS[status] || "Ready", message, STATUS_TONES[status] || "ready");
    // Retry only for transport/provider failures; a clean "no results" is not a
    // failure to retry (STATUS_TONES marks it "error" for emphasis only).
    const retryable = status === "RATE_LIMITED" || status === "NETWORK_ERROR"
      || status === "PARSING_ERROR" || status === "BLOCKED";
    $("btnRetry").hidden = !(retryable && lastRequest);
    if (!retryable) {
      recordQuery(lastRequest?.query || "");
      toggleConfig(true); // search succeeded: give the table the space
    }
    renderResults();
    saveResultsSoon();
    scheduleSave();
    return;
  }
  if (msg.kind === "task_result") {
    finishOperation();
    const op = msg.operation;
    const p = msg.payload;
    if (op === "check") {
      const articles = p.articles ?? [];
      // Write back by stored index.
      if (articles.length === storedArticles.length) {
        storedArticles = articles;
      }
      const checked = articles.filter((a) => (a.pdfStatus ?? a.pdf_status) !== "unknown").length;
      // _finish_check (L1391-1405): detail is the checked-count sentence, and
      // the enrich note is prepended/appended exactly as Python composes it.
      const detail = `${checked} of ${storedArticles.length} records now have a checked public-PDF status.`;
      const note = p.enrich_error ?? "";
      if (p.cancelled) {
        setStatus("Check cancelled", `Collected records were kept. ${detail}` + (note ? ` ${note}` : ""), "warning");
      } else if (note) {
        setStatus("Check finished with errors", `${note} ${detail}`, "warning");
      } else if (p.metadata_only) {
        setStatus("Check complete",
          `Refreshed bibliographic metadata for ${storedArticles.length} records. Public-PDF status was left unchanged.`,
          "success");
      } else {
        setStatus("Check complete",
          `Updated metadata and public-PDF status for ${storedArticles.length} records. ${detail} Records that stay unknown were not verified by the checked sources.`,
          "success");
      }
      renderResults();
    } else if (op === "import") {
      rankingCount = p.count ?? 0;
      $("rankingNotice").textContent = rankingCount > 0 ? `${rankingCount} ranking records` : NO_RANKING_NOTICE;
      const year = p.year;
      // The backend re-labels whatever is on screen; adopt it without a re-search.
      let gained = 0;
      if (Array.isArray(p.articles) && p.articles.length === storedArticles.length) {
        gained = p.articles.filter((a, i) => {
          const before = storedArticles[i] || {};
          const hadSjr = !!(before.sjrQuartile ?? before.sjr_quartile);
          const hadSinta = !!(before.sintaRank ?? before.sinta_rank);
          const hasSjr = !!(a.sjrQuartile ?? a.sjr_quartile);
          const hasSinta = !!(a.sintaRank ?? a.sinta_rank);
          return (!hadSjr && hasSjr) || (!hadSinta && hasSinta);
        }).length;
        storedArticles = p.articles;
      }
      // _finish_import (L1420-1428): filename, cumulative total, and the
      // gained-label sentence always (even "0 displayed record(s)").
      const fileName = String(p.path || "").split(/[\\/]/).pop();
      const yearNote = year
        ? ` Ranking year ${year} was used for records without one.`
        : " Records without a year stay unknown.";
      setStatus("Rankings imported",
        `${p.count} ranking records added from ${fileName}; ${p.total} record(s) loaded in total (earlier imports were kept).` +
        yearNote +
        ` ${gained} displayed record(s) gained a journal label.`,
        "success");
      renderResults();
    } else if (op === "download") {
      const indices = p.indices ?? [];
      const articles = p.articles ?? [];
      // _finish_download (L1444-1458): classify each record as newly saved,
      // retained (path unchanged from a previous run), or not saved this run.
      let newlySaved = 0;
      let retained = 0;
      let failed = 0;
      indices.forEach((storedIndex, i) => {
        const previous = storedArticles[storedIndex];
        const next = articles[i];
        if (!previous || !next) return; // Python skips out-of-range indices entirely
        const nextPath = next.pdfLocalPath ?? next.pdf_local_path;
        const previousPath = previous.pdfLocalPath ?? previous.pdf_local_path;
        if (nextPath) {
          if (nextPath !== previousPath) newlySaved += 1;
          else retained += 1;
        } else {
          failed += 1;
        }
        storedArticles[storedIndex] = next;
      });
      const parts = [newlySaved > 0
        ? `Saved ${newlySaved} new public PDF(s) to ${p.folder}.`
        : "No new public PDF was saved."];
      if (retained > 0) parts.push(` ${retained} record(s) kept the PDF already saved earlier; this attempt did not write a new file.`);
      if (failed > 0) parts.push(` ${failed} selected record(s) had no downloadable public PDF or were cancelled; their metadata was kept.`);
      setStatus("Download finished", parts.join(""), newlySaved > 0 ? "success" : "warning");
      renderResults();
    } else if (op === "citations") {
      if (p.cancelled) {
        setStatus("Citation export cancelled", "No citation file was written.", "warning");
      } else {
        setStatus("Citations exported",
          `Exported ${p.count} citation(s) to ${p.path}. ${p.verified} of ${p.count} resolved via Crossref; unresolved entries keep known fields and a review note.`,
          "success");
        rememberExportFolder(p.path);
      }
    }
    return;
  }
  if (msg.kind === "task_error") {
    finishOperation();
    taskError(msg.operation, msg.message);
  }
}

function taskError(operation, message) {
  // One of the two messagebox paths: dismissable dialog + error status.
  // Verbatim from _handle_task_error (L1350-1360): past-tense failure headings,
  // not the OPERATION_HEADINGS running labels.
  const headings = {
    import: "Ranking import failed",
    download: "Download failed",
    check: "Check failed",
    citations: "Citation export failed",
  };
  const heading = headings[operation] || "Operation failed";
  // L1353: an import failure must say the loaded records survived.
  const detail = operation === "import" ? `${message} Existing ranking records were kept.` : message;
  setStatus(heading, detail, "error");
  showError(heading, detail);
}

function showError(title, detail) {
  $("errorTitle").textContent = title;
  $("errorDetail").textContent = detail;
  const dlg = $("errorDialog");
  // Escape is native dialog behavior; never stack a second modal.
  if (!dlg.open) dlg.showModal();
}

function finishOperation() {
  // running predicate = worker liveness in Python; here: cleared on result.
  // setIdleState() is the single source of truth for enable/disable, so every
  // terminal path (result, task_result, task_error, invoke rejection) ends here
  // instead of each site re-enabling controls by hand.
  running = false;
  currentOperation = null;
  setProgress(0, 100); // a finished operation leaves an empty bar, not a full one
  setIdleState();
  saveSettingsNow();
}

// Unranked journals (2.15): turns "unknown" into a worklist the user can source
// rows for and re-import.
function showGapReport() {
  if (storedArticles.length === 0) {
    setStatus("No results yet", "Run a search before looking for unranked journals.", "warning");
    return;
  }
  const gaps = new Map();
  for (const a of storedArticles) {
    const labelled = (a.sjrQuartile ?? a.sjr_quartile ?? "") || (a.sintaRank ?? a.sinta_rank ?? "");
    if (labelled) continue;
    const journal = (a.journal || "").trim() || "(no journal recorded)";
    const key = journal.toLocaleLowerCase();
    const entry = gaps.get(key) || { journal, issns: new Set(), count: 0 };
    entry.count += 1;
    for (const issn of (a.issns ?? [])) entry.issns.add(issn);
    gaps.set(key, entry);
  }
  const lines = [...gaps.values()]
    .sort((x, y) => y.count - x.count)
    .map((g) => `${g.count}\u00d7  ${g.journal}${g.issns.size ? `  (ISSN ${[...g.issns].join(", ")})` : ""}`);
  $("errorTitle").textContent = "Journals with no imported ranking match";
  $("errorDetail").textContent = gaps.size === 0
    ? "Every stored record already carries a rank label."
    : `${gaps.size} distinct journal(s) across ${storedArticles.length} records have no SJR/SINTA match:\n\n${lines.join("\n")}`;
  if (!$("errorDialog").open) $("errorDialog").showModal();
}

// ---- retry ----
function retryLastSearch() {
  if (running || !lastRequest) return;
  $("query").value = lastRequest.query;
  $("pages").value = String(lastRequest.pages);
  $("folder").value = lastRequest.outputFolder || "";
  $("rankingEnabled").checked = !!lastRequest.rankingEnabled;
  const preset = $("yearPreset");
  const custom = lastRequest.yearFrom != null || lastRequest.yearTo != null;
  preset.selectedIndex = custom ? 4 : 0;
  $("yearFrom").value = lastRequest.yearFrom == null ? "" : String(lastRequest.yearFrom);
  $("yearTo").value = lastRequest.yearTo == null ? "" : String(lastRequest.yearTo);
  onYearPresetChanged();
  startSearch();
}

// ---- collapsible config panel ----
function toggleConfig(collapse) {
  const panel = $("configPanel");
  // toggleConfig() flips; toggleConfig(true) collapses; toggleConfig(false) expands.
  const collapsed = collapse === undefined ? !panel.classList.contains("collapsed") : !!collapse;
  panel.classList.toggle("collapsed", collapsed);
  $("btnToggleConfig").setAttribute("aria-expanded", collapsed ? "false" : "true");
  $("btnToggleConfig").textContent = collapsed ? "Filters \u25b8" : "Filters \u25be";
}

// ---- density + columns ----
function applyDensity() {
  $("resultsTable").classList.toggle("compact", $("compactRows").checked);
}

function syncColumnCheckboxes() {
  for (const box of document.querySelectorAll("#columnsMenu input[data-col]")) {
    box.checked = !hiddenColumns.has(box.dataset.col);
  }
  for (const col of document.querySelectorAll("#resultsTable colgroup col")) {
    col.style.display = hiddenColumns.has(col.dataset.col) ? "none" : "";
  }
  // Headers must follow the colgroup, or the visible headers stop matching cells.
  for (const th of document.querySelectorAll("#resultsTable th[data-key]")) {
    th.style.display = hiddenColumns.has(th.dataset.key) ? "none" : "";
  }
}

// ---- query history ----
function recordQuery(query) {
  const q = (query || "").trim();
  if (!q) return;
  queryHistory = [q, ...queryHistory.filter((x) => x !== q)].slice(0, 20);
  historyCursor = -1;
  const list = $("queryHistory");
  list.innerHTML = "";
  for (const item of queryHistory) {
    const o = document.createElement("option");
    o.value = item;
    list.appendChild(o);
  }
}

// ---- export folder reveal ----
function rememberExportFolder(path) {
  const folder = path && path.includes("\\")
    ? path.slice(0, path.lastIndexOf("\\"))
    : path && path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  lastExportFolder = folder || path || "";
  $("btnReveal").hidden = !lastExportFolder;
  $("menuOpenFolder").disabled = !lastExportFolder;
}

// ---- debounced renders ----
let renderTimer = null;
function scheduleResultsRender() {
  clearTimeout(renderTimer);
  renderTimer = setTimeout(renderResults, 150);
}

// ---- settings persistence ----
const SETTINGS_FIELDS = [
  ["query", "query"], ["pages", "pages"], ["folder", "output_folder"],
  ["rankingPath", "ranking_path"], ["rankingYear", "ranking_year"],
  ["yearFrom", "year_from"], ["yearTo", "year_to"],
];

function collectSettings() {
  return {
    query: $("query").value,
    pages: $("pages").value,
    output_folder: $("folder").value,
    ranking_path: $("rankingPath").value,
    ranking_year: $("rankingYear").value,
    ranking_enabled: $("rankingEnabled").checked,
    year_preset_index: $("yearPreset").selectedIndex || 0,
    year_from: $("yearFrom").value,
    year_to: $("yearTo").value,
    pdf_filter: $("pdfFilter").value || "All",
    sort_key: sortKey || "",
    sort_dir: sortDir,
    compact_rows: $("compactRows").checked,
    hidden_columns: [...hiddenColumns],
    query_history: queryHistory,
  };
}

let saveTimer = null;
function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(saveSettingsNow, 500);
}

function saveSettingsNow() {
  clearTimeout(saveTimer);
  saveTimer = null;
  invoke("save_settings", { settings: collectSettings() }).catch(() => {});
}

async function loadSettings() {
  let saved;
  try {
    saved = await invoke("load_settings");
  } catch {
    return;
  }
  if (!saved) return;
  // Commands now return { settings, notice }; a bare settings object is still
  // accepted so an older backend keeps working.
  const notice = saved.notice ?? "";
  if (saved.settings) saved = saved.settings;
  if (notice) setStatus("Settings reset", notice, "warning");
  $("query").value = saved.query ?? "";
  $("pages").value = saved.pages || "1";
  $("folder").value = saved.output_folder ?? "";
  $("rankingPath").value = saved.ranking_path ?? "";
  $("rankingYear").value = saved.ranking_year ?? "";
  $("rankingEnabled").checked = saved.ranking_enabled !== false;
  $("yearFrom").value = saved.year_from ?? "";
  $("yearTo").value = saved.year_to ?? "";
  const idx = saved.year_preset_index ?? 0;
  const preset = $("yearPreset");
  preset.selectedIndex = idx >= 0 && idx < preset.children.length ? idx : 0;
  const filterValue = saved.pdf_filter || "All";
  if (PDF_FILTERS.includes(filterValue)) {
    $("pdfFilter").value = filterValue;
    pdfFilter = filterValue;
  }
  // v3.2.0 state
  hiddenColumns = new Set(saved.hidden_columns ?? []);
  $("compactRows").checked = !!saved.compact_rows;
  applyDensity();
  syncColumnCheckboxes();
  queryHistory = saved.query_history ?? [];
  historyCursor = -1;
  $("queryHistory").innerHTML = "";
  for (const item of queryHistory) {
    const o = document.createElement("option");
    o.value = item;
    $("queryHistory").appendChild(o);
  }
  if (saved.sort_key) { sortKey = saved.sort_key; sortDir = saved.sort_dir === -1 ? -1 : 1; }
  // A catalog restored at startup means ranks are already available; the
  // notice must say so instead of claiming nothing was imported.
  try {
    const restored = await invoke("load_catalog_records");
    if (Array.isArray(restored) && restored.length > 0) {
      rankingCount = restored.length;
      $("rankingNotice").textContent = `${rankingCount} ranking records`;
    }
  } catch {
    /* no catalog is a normal first run */
  }
  onYearPresetChanged();
  renderResults();
  restoreResults();
}

async function restoreResults() {
  if (running || storedArticles.length > 0) return;
  let articles = [];
  try { articles = await invoke("load_results"); } catch { return; }
  if (!Array.isArray(articles) || articles.length === 0) return;
  const word = articles.length === 1 ? "result" : "results";
  storedArticles = articles;
  summaryBase = `${articles.length} ${word} restored from the last session`;
  renderResults();
}

let resultsTimer = null;
function saveResultsSoon() {
  clearTimeout(resultsTimer);
  resultsTimer = setTimeout(() => {
    if (storedArticles.length === 0) return;
    invoke("save_results", { articles: storedArticles }).catch(() => {});
  }, 800);
}

// ---- helpers ----
async function dialog(kind, title) {
  // File/dir dialogs via the browser layer (Tauri dialog API or fallback).
  try {
    return await invoke("pick_path", { kind, title });
  } catch {
    return null;
  }
}

function invoke(cmd, args = {}) {
  const fn = window.__TAURI__?.core?.invoke || window.__TAURI__?.invoke;
  if (!fn) return Promise.reject(`invoke unavailable for ${cmd}`);
  return fn(cmd, args);
}

async function browseFolder() {
  const folder = await dialog("pickFolder", "");
  if (folder) $("folder").value = folder;
}

// Listen for worker events (Tauri event system).
function listenEvents() {
  const ev = window.__TAURI__?.event?.listen;
  if (!ev) return;
  ev("worker", (event) => onWorkerMessage(event.payload));
}

init();
listenEvents();
