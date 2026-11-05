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
// UI-only provenance for the *executed* search. A restored session has none,
// so the footer must never infer search bounds from editable form fields.
let displayedSearchRequest = null;
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
let detailIndex = -1; // record shown in the inspector
let inspectorGeneration = 0; // guards late async inspector loads
let reviewDraft = null; // snapshot of unsaved inspector edits
let currentView = "all"; // sidebar quick view (mirrors decisionFilter)
let retryableResult = false; // last search ended in a retryable failure
let queryHistory = []; // recent successful queries, newest first
let historyCursor = -1; // recall position inside queryHistory
let hiddenColumns = new Set(); // column keys hidden by the Columns menu
let decisionFilter = "all"; // all | undecided | include | exclude | maybe
let tagFilter = ""; // free-text tag match
let savedFilters = {}; // name -> {pdf, decision, tag, query}
const ALL_COLUMNS = ["title", "authors", "decision", "pdf", "index", "rank", "cited", "oa", "tags", "link"];
const DECISION_VALUES = ["all", "undecided", "include", "exclude", "maybe"];

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
  // First-paint theme: the stylesheet defines light tokens on :root, but the
  // stored preference must apply before any await so dark users see no flash.
  applyTheme("system");

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

  // ---- search ----
  $("btnSearch").onclick = startSearch;
  $("btnCancel").onclick = cancelOperation;
  $("btnToggleConfig").onclick = () => toggleConfig();
  $("btnRetry").onclick = retryLastSearch;
  $("query").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); startSearch(); }
  });

  // ---- library navigation ----
  for (const button of document.querySelectorAll(".sidebar .nav-item[data-view]")) {
    button.onclick = () => setDecisionFilter(button.dataset.view);
  }
  $("viewTitle").onclick = () => {};

  // ---- menus ----
  bindPopover("btnAddMenu", "addMenu");
  bindPopover("btnReviewMenu", "reviewMenu");
  bindPopover("btnActionsMenu", "actionsMenu");
  bindPopover("btnView", "columnsMenu");
  $("btnFilters").onclick = () => toggleFiltersPopover();

  // ---- collection actions ----
  $("btnCheck").onclick = () => { closeMenus(); checkMetadataPdfs(false); };
  $("btnMetadataOnly").onclick = () => { closeMenus(); checkMetadataPdfs(true); };
  $("btnImport").onclick = importRanking;
  $("btnGapReport").onclick = () => { closeMenus(); showGapReport(); };
  $("btnBrowse").onclick = browseFolder;
  $("btnSettings").onclick = () => $("settingsDialog").showModal();
  $("btnCloseSettings").onclick = () => $("settingsDialog").close();

  // ---- review tools ----
  const bindReview = (id, fn) => {
    $(id).onclick = () => { closeMenus(); fn(); };
  };
  bindReview("menuPrisma", showPrisma);
  bindReview("menuScreeningLog", exportScreeningLog);
  bindReview("menuNearDup", showNearDuplicates);
  bindReview("menuBaselineSave", saveBaseline);
  bindReview("menuBaselineDiff", diffBaseline);
  bindReview("menuRescore", rescoreWithQuery);
  bindReview("menuImportExport", importOwnExport);
  bindReview("menuPasteDois", openPasteDois);
  bindReview("menuCiteForward", () => citationChase("citations"));
  bindReview("menuCiteBackward", () => citationChase("references"));
  $("btnPasteDoisCancel").onclick = () => $("pasteDialog").close();
  $("btnPasteDoisOk").onclick = addPastedDois;

  // ---- selection bar ----
  $("btnInclude").onclick = () => setDecision("include");
  $("btnMaybe").onclick = () => setDecision("maybe");
  $("btnExclude").onclick = () => setDecision("exclude");
  $("btnDownload").onclick = downloadSelected;
  $("btnDetails").onclick = openDetail;
  $("btnClearSelection").onclick = () => clearSelection();
  $("btnExportSelected").onclick = () => openExportDialog(null, "selected");
  $("selectVisible").onchange = () => {
    const ids = visibleIndices();
    if ($("selectVisible").checked) ids.forEach((i) => selection.add(i));
    else ids.forEach((i) => selection.delete(i));
    updateSelectionSurface();
  };

  // ---- table ----
  $("tableBody").onclick = onTableClick;
  $("resultsTable").addEventListener("keydown", onTableKeyDown);
  $("resultsTable").addEventListener("focus", () => {
    if (focusRow === -1) { const ids = visibleIndices(); if (ids.length) { focusRow = ids[0]; renderResults(); focusGridRow(); } }
  });
  $("tableBody").ondblclick = onTableDblClick;
  $("tableBody").oncontextmenu = onTableContextMenu;
  for (const th of document.querySelectorAll("th.sortable")) {
    th.onclick = () => onHeaderClick(th.dataset.key);
  }
  $("btnResetSort").onclick = () => { closeMenus(); resetSort(); };

  // ---- filters ----
  $("pdfFilter").onchange = () => { renderResults(); scheduleSave(); };
  $("decisionFilter").onchange = () => setDecisionFilter($("decisionFilter").value);
  $("tagFilter").oninput = () => { tagFilter = $("tagFilter").value; scheduleResultsRender(); scheduleSave(); };
  $("resultsSearch").oninput = () => {
    resultsQuery = $("resultsSearch").value.trim();
    scheduleResultsRender();
  };
  $("btnSaveFilter").onclick = saveCurrentFilter;
  $("btnClearFilters").onclick = () => { closeFiltersPopover(); clearResultFilters(); };
  $("savedFilters").onchange = () => {
    const name = $("savedFilters").value;
    if (name && savedFilters[name]) applyFilter(savedFilters[name]);
    $("savedFilters").value = "";
  };
  $("btnEmptyClear").onclick = clearResultFilters;
  $("btnEmptyPrimary").onclick = () => $("query").focus();

  // ---- view options ----
  $("compactRows").onchange = () => { applyDensity(); scheduleSave(); };
  for (const box of document.querySelectorAll("#columnsMenu input[data-col]")) {
    box.onchange = () => {
      const key = box.dataset.col;
      if (box.checked) hiddenColumns.delete(key); else hiddenColumns.add(key);
      renderResults();
      scheduleSave();
    };
  }

  // ---- inspector ----
  $("btnInspectorClose").onclick = requestInspectorClose;
  $("btnInspectorPrev").onclick = () => stepInspector(-1);
  $("btnInspectorNext").onclick = () => stepInspector(1);
  $("inspectorScrim").onclick = requestInspectorClose;
  $("btnDetailSaveReview").onclick = () => saveDetailReview({ close: false });
  $("btnDetailOpenLink").onclick = () => { const a = storedArticles[detailIndex]; if (a) openRecordLink(a); };
  $("btnDetailOpenPdf").onclick = () => { const a = storedArticles[detailIndex]; if (a) revealRecordPdf(a); };
  $("btnDetailCopyDoi").onclick = () => { const a = storedArticles[detailIndex]; if (a) copyText(a.doi || ""); };
  $("btnDetailCopyTitle").onclick = () => { const a = storedArticles[detailIndex]; if (a) copyText(a.title || ""); };
  $("btnDetailAbstract").onclick = () => { if (detailIndex !== -1) openAbstract(detailIndex); };
  for (const id of INSPECTOR_FIELDS) {
    $(id).addEventListener("input", markDraftState);
    $(id).addEventListener("change", markDraftState);
  }
  $("detailDecision").addEventListener("change", () => {
    $("detailReasonField").hidden = $("detailDecision").value !== "exclude";
  });
  $("detailTags").addEventListener("input", updateTagPreview);

  // ---- export ----
  $("btnExportMenu").onclick = () => openExportDialog();
  $("btnCloseExport").onclick = () => $("exportDialog").close();
  $("btnDoExport").onclick = runExport;
  $("exportFormat").onchange = updateExportFields;
  $("exportScope").onchange = updateExportFields;
  $("btnReveal").onclick = () => {
    if (lastExportFolder) invoke("reveal_path", { path: lastExportFolder }).catch(() => {});
  };

  // ---- dialogs ----
  $("btnErrorClose").onclick = () => $("errorDialog").close();
  $("btnCloseReport").onclick = () => $("reportDialog").close();

  // ---- theme ----
  $("themeSelect").onchange = () => { applyTheme($("themeSelect").value); scheduleSave(); };
  if (window.matchMedia) {
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const listener = () => { if (($("themeSelect").value || "system") === "system") applyTheme("system"); };
    if (query.addEventListener) query.addEventListener("change", listener);
    else if (query.addListener) query.addListener(listener);
  }

  document.addEventListener("click", closeMenus);
  document.addEventListener("keydown", onKeyDown);
  $("recordInspector").addEventListener("keydown", trapInspectorFocus);
  window.addEventListener("resize", updateInspectorMode);
  for (const id of ["pages", "rankingPath", "rankingYear", "yearFrom", "yearTo"]) {
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
    invoke("save_window_geometry").catch(() => {});
  });
  setIdleState();
  loadSettings();
}

// ---- popover registry ----
const POPOVERS = [
  ["btnAddMenu", "addMenu"],
  ["btnReviewMenu", "reviewMenu"],
  ["btnActionsMenu", "actionsMenu"],
  ["btnView", "columnsMenu"],
];

function bindPopover(buttonId, menuId) {
  const button = $(buttonId);
  const menu = $(menuId);
  if (!button || !menu) return;
  button.setAttribute("aria-expanded", "false");
  button.onclick = (e) => {
    e.stopPropagation();
    const willOpen = menu.hidden;
    closeMenus();
    if (!willOpen) return;
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    const rect = button.getBoundingClientRect();
    menu.style.top = `${Math.round(rect.bottom + 6)}px`;
    menu.style.left = `${Math.round(Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8))}px`;
    clampToViewport(menu);
    const first = menu.querySelector("button:not(:disabled), input");
    if (first) first.focus();
  };
}

function clampToViewport(el) {
  const rect = el.getBoundingClientRect();
  const overflowY = rect.bottom - (window.innerHeight - 8);
  if (overflowY > 0) el.style.top = `${Math.max(8, rect.top - overflowY)}px`;
  const overflowX = rect.right - (window.innerWidth - 8);
  if (overflowX > 0) el.style.left = `${Math.max(8, rect.left - overflowX)}px`;
}

function toggleFiltersPopover() {
  const popover = $("filtersPopover");
  const button = $("btnFilters");
  const willOpen = popover.hidden;
  closeMenus();
  if (!willOpen) return;
  popover.hidden = false;
  button.setAttribute("aria-expanded", "true");
  const rect = button.getBoundingClientRect();
  popover.style.top = `${Math.round(rect.bottom + 6)}px`;
  popover.style.left = `${Math.round(Math.min(rect.left, window.innerWidth - popover.offsetWidth - 8))}px`;
  clampToViewport(popover);
  $("pdfFilter").focus();
}

function closeFiltersPopover() {
  const popover = $("filtersPopover");
  if (popover && !popover.hidden) {
    popover.hidden = true;
    $("btnFilters").setAttribute("aria-expanded", "false");
    return true;
  }
  return false;
}

// ---- keyboard ----
function onKeyDown(e) {
  const active = document.activeElement;
  const inField = active && active.closest("input, textarea, select");
  if (e.key === "Escape") {
    // Precedence: popovers, then the inspector (via its draft guard), then a
    // running cancellable operation, then the selection.
    if (closeMenus()) {
      e.preventDefault();
      return;
    }
    const topDialog = document.querySelector("dialog[open]");
    if (topDialog) return; // native dialogs handle their own Escape
    if (!$("recordInspector").hidden) {
      e.preventDefault();
      requestInspectorClose();
      return;
    }
    if (running && WORKER_OPERATIONS.has(currentOperation)) {
      cancelOperation();
      return;
    }
    if (selection.size > 0) {
      e.preventDefault();
      clearSelection();
    }
    return;
  }
  if (running) return;
  if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "e") {
    e.preventDefault();
    openExportDialog("bibtex");
    return;
  }
  if (e.ctrlKey && !e.altKey && e.shiftKey && e.key.toLowerCase() === "a") {
    e.preventDefault();
    clearSelection();
    return;
  }
  if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "a" && !inField) {
    e.preventDefault();
    for (const [i] of visibleArticles()) selection.add(i);
    updateSelectionSurface();
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

/// Drop the selection and refresh only the selection-dependent surface.
function clearSelection() {
  selection.clear();
  updateSelectionSurface();
  syncControls();
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
  if (!menu) return;
  menu.hidden = !open;
  if (btn) {
    btn.classList.toggle("open", open);
    btn.setAttribute("aria-expanded", open ? "true" : "false");
  }
}

/// Dismiss every popover/menu; returns true when something actually closed.
function closeMenus() {
  let closed = false;
  for (const [buttonId, menuId] of POPOVERS) {
    const menu = $(menuId);
    if (menu && !menu.hidden) {
      menu.hidden = true;
      $(buttonId).setAttribute("aria-expanded", "false");
      closed = true;
    }
  }
  if (closeFiltersPopover()) closed = true;
  const ctx = document.querySelector(".ctxmenu");
  if (ctx) {
    ctx.remove();
    ctxTarget = null;
    closed = true;
  }
  return closed;
}

// ---- sortable columns ----
// 5.8: sortable headers cycle asc/desc; this returns to the original order.
function resetSort() {
  sortKey = null;
  sortDir = 1;
  renderResults();
  scheduleSave();
}

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
    case "decision":
      return decisionLabel(a);
    case "tags":
      return tagsLabel(a).toLocaleLowerCase();
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
const WORKER_OPERATIONS = new Set(["search", "check", "import", "download", "citations"]);

function setRunning(operation) {
  running = true;
  currentOperation = operation;
  closeMenus();
  syncControls();
}

/// Begin/end of the IDLE state. The ONLY writer of `running`/`currentOperation`
/// besides setRunning(); must never render (v3.0.0 recursion came from here).
function setIdleState() {
  running = false;
  currentOperation = null;
  syncControls();
}

/// Control state derived from (running, collection, selection, scope).
/// Never mutates those values, never renders: rendering calls this, so a mere
/// repaint can no longer fake operation completion.
function syncControls() {
  const hasArticles = storedArticles.length > 0;
  const hasVisible = visibleArticles().length > 0;
  const cancellable = running && WORKER_OPERATIONS.has(currentOperation);
  const busy = running;
  const selectedCount = selection.size;

  $("btnSearch").disabled = busy;
  $("btnCancel").disabled = !cancellable;
  $("btnCancel").hidden = !cancellable;
  $("btnCheck").disabled = busy || !hasArticles;
  $("btnMetadataOnly").disabled = busy || !hasArticles;
  $("btnDownload").disabled = busy || selectedCount === 0;
  $("btnDetails").disabled = selectedCount !== 1;
  $("btnImport").disabled = busy;
  $("btnBrowse").disabled = busy;
  $("btnExportMenu").disabled = busy || !hasVisible;
  $("btnExportSelected").disabled = busy || selectedCount === 0;
  $("btnDoExport").disabled = busy || exportSelectionCount() === 0;
  $("btnRetry").hidden = !(retryableResult && lastRequest && !busy);
  $("btnReveal").hidden = !lastExportFolder;
  $("menuImportExport").disabled = busy;
  $("menuPasteDois").disabled = busy;
  $("menuRescore").disabled = busy || !hasArticles;
  $("menuBaselineSave").disabled = busy || !hasArticles;
  $("menuBaselineDiff").disabled = busy || !hasArticles;
  $("menuScreeningLog").disabled = !hasArticles;
  $("menuPrisma").disabled = !hasArticles;
  $("menuNearDup").disabled = !hasArticles;

  $("query").disabled = busy;
  $("pages").disabled = busy;
  $("folder").disabled = busy;
  $("rankingPath").disabled = busy;
  $("rankingYear").disabled = busy;
  $("rankingEnabled").disabled = busy;
  $("pdfFilter").disabled = false;
  $("yearPreset").disabled = busy;
  const customYear = $("yearPreset").selectedIndex === 4;
  $("yearFrom").disabled = busy || !customYear;
  $("yearTo").disabled = busy || !customYear;

  syncColumnCheckboxes();
  updateSelectionSurface();
  updateAppliedYear();
  updateViewTitle();
}

/// Single commit point for every change to `storedArticles`.
/// modes: replace (new result set), append (merge), reorder (same records, new
/// order), update (same order, changed fields).
function applyArticleCollection(articles, mode, options = {}) {
  const previous = storedArticles;
  const prevSelection = new Set(selection);
  const prevDetailKey = detailIndex >= 0 ? reviewKey(previous[detailIndex]) : "";

  if (mode === "replace") {
    storedArticles = Array.isArray(articles) ? articles : [];
    selection.clear();
    focusRow = -1;
    anchorRow = -1;
    ctxTarget = null;
    detailIndex = -1;
    closeInspector({ force: true });
  } else if (mode === "append") {
    storedArticles = Array.isArray(articles) ? articles : [];
    const byKey = new Map();
    storedArticles.forEach((a, i) => {
      const key = reviewKey(a);
      if (key) byKey.set(key, i);
    });
    const remapped = new Set();
    for (const index of prevSelection) {
      const key = reviewKey(previous[index]);
      const next = key ? byKey.get(key) : undefined;
      if (next !== undefined) remapped.add(next);
    }
    selection.clear();
    remapped.forEach((i) => selection.add(i));
    focusRow = -1;
    anchorRow = -1;
    if (prevDetailKey && byKey.has(prevDetailKey)) detailIndex = byKey.get(prevDetailKey);
    else detailIndex = -1;
  } else if (mode === "reorder") {
    storedArticles = Array.isArray(articles) ? articles : [];
    const byKey = new Map();
    storedArticles.forEach((a, i) => {
      const key = reviewKey(a);
      if (key) byKey.set(key, i);
    });
    const remapped = new Set();
    for (const index of prevSelection) {
      const key = reviewKey(previous[index]);
      const next = key ? byKey.get(key) : undefined;
      if (next !== undefined) remapped.add(next);
    }
    selection.clear();
    remapped.forEach((i) => selection.add(i));
    focusRow = -1;
    anchorRow = -1;
    if (prevDetailKey && byKey.has(prevDetailKey)) detailIndex = byKey.get(prevDetailKey);
    else detailIndex = -1;
  } else if (mode === "update") {
    const sameLength = Array.isArray(articles) && articles.length === storedArticles.length;
    storedArticles = Array.isArray(articles) ? articles : storedArticles;
    if (!sameLength) {
      const kept = new Set();
      for (const index of prevSelection) if (index < storedArticles.length) kept.add(index);
      selection.clear();
      kept.forEach((i) => selection.add(i));
      if (detailIndex >= storedArticles.length) detailIndex = -1;
    }
  }

  if (options.summary !== undefined) summaryBase = options.summary;
  renderResults();
  if (mode === "replace" || mode === "append" || mode === "reorder") {
    if (detailIndex >= 0 && options.refreshInspector !== false) refreshInspector();
  } else if (detailIndex >= 0 && options.refreshInspector !== false) {
    refreshInspector();
  }
  if (options.persist !== false) saveResultsSoon();
}

// ---- selection surface ----
/// Which of the current export scope's papers exist.
function exportSelectionCount() {
  const scope = $("exportScope") ? $("exportScope").value : "visible";
  if (scope === "selected") return selection.size;
  if (scope === "all") return storedArticles.length;
  return visibleArticles().length;
}

/// Reflect selection into rows, checkboxes, summary and the bulk bar without
/// rebuilding the table. Every mouse/keyboard/collection path ends here so
/// aria-selected and the count can never disagree with the CSS highlight.
function updateSelectionSurface() {
  const rows = document.querySelectorAll("#tableBody tr[data-iid]");
  let visibleSelected = 0;
  let visibleTotal = 0;
  for (const tr of rows) {
    const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
    const selected = selection.has(index);
    tr.classList.toggle("selected", selected);
    tr.setAttribute("aria-selected", selected ? "true" : "false");
    const box = tr.querySelector("input.row-select");
    if (box) box.checked = selected;
    visibleTotal += 1;
    if (selected) visibleSelected += 1;
  }

  const bar = $("selectionBar");
  if (bar) {
    const count = selection.size;
    bar.hidden = count === 0;
    $("selCount").textContent = `${count} selected`;
    const outside = count - visibleSelected;
    const outsideLabel = $("selOutside");
    if (outsideLabel) {
      outsideLabel.hidden = outside <= 0;
      outsideLabel.textContent = outside > 0 ? `${outside} outside this view` : "";
    }
  }

  const selectAll = $("selectVisible");
  if (selectAll) {
    selectAll.checked = visibleTotal > 0 && visibleSelected === visibleTotal;
    selectAll.indeterminate = visibleSelected > 0 && visibleSelected < visibleTotal;
    selectAll.disabled = visibleTotal === 0;
  }

  if ($("btnDetails")) $("btnDetails").disabled = running || selection.size !== 1;
  if ($("btnDownload")) $("btnDownload").disabled = running || selection.size === 0;
  if ($("btnExportSelected")) $("btnExportSelected").disabled = running || selection.size === 0;
  if ($("btnDoExport")) $("btnDoExport").disabled = running || exportSelectionCount() === 0;
  refreshSummary();
}

function updateViewTitle() {
  const labels = {
    all: "All papers",
    undecided: "To screen",
    include: "Included",
    exclude: "Excluded",
    maybe: "Maybe",
  };
  const title = $("viewTitle");
  if (title) title.textContent = labels[currentView] || "All papers";
  for (const button of document.querySelectorAll(".sidebar .nav-item[data-view]")) {
    button.setAttribute("aria-current", button.dataset.view === currentView ? "true" : "false");
  }
}

/// Sidebar counts always describe the whole collection, never the filtered view.
function refreshViewCounts() {
  const counts = { all: storedArticles.length, undecided: 0, include: 0, exclude: 0, maybe: 0 };
  for (const article of storedArticles) {
    const decision = (reviewEntry(article).decision || "").trim();
    if (decision === "include") counts.include += 1;
    else if (decision === "exclude") counts.exclude += 1;
    else if (decision === "maybe") counts.maybe += 1;
    else counts.undecided += 1;
  }
  const set = (id, value) => { const el = $(id); if (el) el.textContent = String(value); };
  set("countAll", counts.all);
  set("countUndecided", counts.undecided);
  set("countInclude", counts.include);
  set("countMaybe", counts.maybe);
  set("countExclude", counts.exclude);
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
    showSearchOptions();
    setStatus("Check search setup", "Page count must be a whole number.", "error");
    return;
  }
  if (pagesValue <= 0) {
    showSearchOptions();
    setStatus("Check search setup", "Page count must be greater than zero.", "error");
    return;
  }
  const query = $("query").value.trim();
  if (!query) {
    setStatus("Check search setup", "Enter a search query first.", "error");
    $("query").focus();
    return;
  }
  // Replacing the collection would strand an unsaved inspector draft.
  const draftChoice = await resolveReviewDraft();
  if (draftChoice === "keep") return;
  if (draftChoice === "save") {
    const saved = await saveDetailReview({ close: false });
    if (!saved) return;
  }
  if (running) return;

  const { from, to } = currentYearBounds();
  const request = {
    query,
    pages: pagesValue,
    rankingEnabled: $("rankingEnabled").checked,
    outputFolder: exportFolder(),
    yearFrom: from,
    yearTo: to,
  };
  lastRequest = { ...request };
  retryableResult = false;
  setProgress(0, 100);
  setStatus(OPERATION_HEADINGS.search, "Results will appear here as soon as the search completes.", "running");
  setRunning("search");
  try {
    await invoke("search_scholar", request);
  } catch (err) {
    setIdleState();
    setStatus("Check search setup", errMessage(err), "error");
  }
}

/// The advanced options are only meaningful for a search, so a validation
/// failure must reveal them instead of pointing at hidden inputs.
function showSearchOptions() {
  const panel = $("configPanel");
  if (!panel || !panel.hidden) return;
  toggleConfig(false);
}

function cancelOperation() {
  if (!running || !WORKER_OPERATIONS.has(currentOperation)) return;
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
    await invoke("check_metadata_pdfs", { articles: storedArticles, metadataOnly });
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
    await invoke("import_ranking", { articles: storedArticles, path, defaultYear: year });
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
    await invoke("download_pdfs", { articles: storedArticles, indices: [...selection], folder });
  } catch (err) {
    finishOperation();
    taskError("download", errMessage(err));
  }
}

// ---- export dialog ----
const EXPORT_STYLE_FORMATS = new Set(["csl-json", "bibtex-stable", "bibtex-files", "grouped", "clipboard"]);
const EXPORT_GROUP_FORMATS = new Set(["excel", "grouped", "clipboard"]);
const EXPORT_ALWAYS_STYLE = new Set(["bibtex", "ris"]);

function openExportDialog(format = null, scope = null) {
  if (running) return;
  if (storedArticles.length === 0) {
    setStatus("No results to export.", "Search or add papers before exporting.", "warning");
    return;
  }
  if (format) $("exportFormat").value = format;
  if (scope) $("exportScope").value = scope;
  if ($("exportFormat").value === "grouped" || $("exportFormat").value === "clipboard") {
    if (!$("groupBy").value) $("groupBy").value = "year";
  }
  updateExportFields();
  const dialog = $("exportDialog");
  if (!dialog.open) dialog.showModal();
  $("exportFormat").focus();
}

/// Scope labels carry live counts so "Selected" can never silently mean
/// "everything" when nothing is chosen.
function updateExportFields() {
  const format = $("exportFormat").value;
  const scope = $("exportScope").value;
  const visible = visibleArticles().length;
  const selected = selection.size;
  const all = storedArticles.length;
  const options = $("exportScope").options;
  options[0].textContent = `Filtered papers (${visible})`;
  options[1].textContent = `Selected papers (${selected})`;
  options[2].textContent = `All collected papers (${all})`;

  const styleField = $("citeStyle").closest(".field");
  const groupField = $("groupBy").closest(".field");
  styleField.hidden = !(EXPORT_STYLE_FORMATS.has(format) || EXPORT_ALWAYS_STYLE.has(format));
  groupField.hidden = !EXPORT_GROUP_FORMATS.has(format);

  const count = scope === "selected" ? selected : scope === "all" ? all : visible;
  const folder = exportFolder() || "your document folder";
  $("exportSummary").textContent = count === 0
    ? "Nothing to export for this scope."
    : `Exporting ${count} ${count === 1 ? "paper" : "papers"} to ${folder}.`;
  $("exportNote").textContent = (format === "bibtex" || format === "ris")
    ? "Records are resolved against Crossref when possible. Unresolved records keep their known fields and carry a review note."
    : format === "clipboard"
      ? "Also writes scholar_bibliography.md beside the destination."
      : format === "csv"
        ? "CSV keeps the fixed 20-column contract used by the Python implementation."
        : "";
  $("btnDoExport").disabled = running || count === 0;
  $("btnDoExport").textContent = format === "clipboard" ? "Copy references" : "Export";
}

async function runExport() {
  if (running) return;
  const format = $("exportFormat").value;
  const scope = $("exportScope").value;
  const records = exportRecords(scope);
  if (records.length === 0) {
    setStatus("No results to export.", "Choose a scope with at least one paper.", "warning");
    return;
  }
  const dialog = $("exportDialog");
  try {
    if (format === "csv" || format === "excel") {
      await exportSpreadsheet(format, records);
    } else if (format === "bibtex" || format === "ris") {
      await startCitationExport(format, records);
    } else if (format === "clipboard") {
      await copyBibliography(records);
    } else if (Object.prototype.hasOwnProperty.call(EXTRA_FORMATS, format)) {
      await extraExport(format, records);
    } else if (Object.prototype.hasOwnProperty.call(EXTRA_CITATIONS, format)) {
      await extraCitationExport(format, records);
    } else {
      throw new Error(`Unknown export format: ${format}`);
    }
  } catch (err) {
    // Keep the dialog open so the user can retry without re-choosing.
    return;
  }
  if (dialog.open) dialog.close();
}

async function startCitationExport(format, records) {
  if (running) return;
  if (!Array.isArray(records) || records.length === 0) {
    setStatus("No results to export.", "Choose a scope with at least one paper.", "warning");
    return;
  }
  const folder = exportFolder();
  const filename = format === "bibtex" ? DEFAULT_EXPORT_NAMES.bibtex : DEFAULT_EXPORT_NAMES.ris;
  const path = joinPath(folder, filename);
  setStatus(OPERATION_HEADINGS.citations, "Resolving citations against Crossref, then writing the file.", "running");
  setRunning("citations");
  try {
    await invoke("export_citations", { articles: records, format, path });
  } catch (err) {
    finishOperation();
    taskError("citations", errMessage(err));
    throw err;
  }
  return path;
}

// ---- spreadsheet exports (synchronous on the UI thread) ----
async function exportSpreadsheet(kind, records = null) {
  const visible = Array.isArray(records) ? records : exportRecords();
  if (visible.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const folder = exportFolder();
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
    throw err;
  }
}

// ---- table rendering (iid = article-{storedIndex}) ----
/// True when any result filter narrows the collection.
function anyFilterActive() {
  return pdfFilter !== "All" || resultsQuery !== "" || decisionFilter !== "all" || tagFilter.trim() !== "";
}

/// True when the record passes the decision/tag filters.
function passesReviewFilters(a) {
  if (decisionFilter !== "all") {
    const decision = decisionLabel(a) || "undecided";
    if (decision !== decisionFilter) return false;
  }
  const wanted = tagFilter.trim().toLocaleLowerCase();
  if (wanted) {
    const tags = (reviewEntry(a).tags || []).map((t) => t.toLocaleLowerCase());
    if (!tags.some((t) => t.includes(wanted))) return false;
  }
  return true;
}

function filteredArticles() {
  const needle = resultsQuery.toLocaleLowerCase();
  const rows = storedArticles
    .map((a, i) => [i, a])
    .filter(([, a]) => {
      if (!passesReviewFilters(a)) return false;
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
  body.innerHTML = "";
  for (const [index, a] of rows) {
    const tr = document.createElement("tr");
    tr.dataset.iid = `article-${index}`;
    tr.tabIndex = -1;
    tr.setAttribute("role", "row");
    tr.setAttribute("aria-selected", selection.has(index) ? "true" : "false");
    if (selection.has(index)) tr.classList.add("selected");
    if (index === focusRow) tr.classList.add("focused");

    const title = a.title ?? "";
    const authors = a.authors ?? "";
    const rank = [a.sjrQuartile ?? a.sjr_quartile, a.sintaRank ?? a.sinta_rank]
      .filter(Boolean).join(" / ");
    const status = a.pdfStatus ?? a.pdf_status;
    const localPath = a.pdfLocalPath ?? a.local_path ?? a.pdf_local_path;
    const pdfUrl = a.pdfUrl ?? a.pdf_url ?? "";
    const link = a.link ?? "";
    const year = a.publicationYear ?? a.publication_year;
    const journal = (a.journal ?? "").trim();

    // Checkbox cell: outside ALL_COLUMNS and the export mapping.
    const selectCell = document.createElement("td");
    selectCell.dataset.col = "select";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "row-select";
    box.checked = selection.has(index);
    box.setAttribute("aria-label", `Select ${title || "paper"}`);
    box.dataset.index = String(index);
    selectCell.appendChild(box);
    tr.appendChild(selectCell);

    const cells = [
      ["title", title, null],
      ["authors", authors, null],
      ["decision", decisionLabel(a), null],
      ["pdf", status === "available" ? (localPath ? "Downloaded" : "Public PDF")
        : status === "unavailable" ? "No public PDF" : "Unknown",
        localPath ? { action: "reveal-pdf", value: localPath, label: "Downloaded" }
          : pdfUrl ? { action: "open-pdf", value: pdfUrl, label: "Public PDF" } : null],
      ["index", indexingLabel(a), null],
      ["rank", rank, null],
      ["cited", (a.citedBy ?? a.cited_by) == null ? "" : String(a.citedBy ?? a.cited_by), null],
      ["oa", (a.oaStatus ?? a.oa_status ?? "").trim(), null],
      ["tags", tagsLabel(a), null],
      ["link", link ? "Open" : "No link",
        link ? { action: "open-link", value: link, label: "Open" } : null],
    ];

    for (const [key, text, action] of cells) {
      if (hiddenColumns.has(key)) continue;
      const td = document.createElement("td");
      td.dataset.col = key;
      td.title = text; // full value survives truncation

      if (key === "title") {
        const wrapper = document.createElement("div");
        wrapper.className = "cell-title";
        wrapper.textContent = title;
        td.textContent = "";
        td.appendChild(wrapper);
        const sub = [journal, year].filter((v) => v !== "" && v != null).join(" · ");
        if (sub) {
          // Venue line stays available without a separate Journal column.
          const subEl = document.createElement("div");
          subEl.className = "cell-sub";
          subEl.textContent = sub;
          td.appendChild(subEl);
        }
        const notice = (a.retraction ?? "").trim();
        if (notice) {
          const badge = document.createElement("span");
          badge.className = "badge retracted";
          badge.textContent = notice === "retraction" ? "RETRACTED" : notice.toUpperCase();
          badge.title = (a.retractionDoi ?? a.retraction_doi)
            ? `Notice DOI: ${a.retractionDoi ?? a.retraction_doi}`
            : "This record carries a correction notice";
          wrapper.appendChild(document.createTextNode(" "));
          wrapper.appendChild(badge);
        }
      } else if (key === "decision") {
        const decision = decisionLabel(a);
        if (decision) {
          const badge = document.createElement("span");
          badge.className = `badge ${decision}`;
          badge.textContent = decision.toUpperCase();
          td.appendChild(badge);
        } else {
          td.textContent = "";
        }
      } else if (key === "tags") {
        td.textContent = "";
        for (const tag of reviewEntry(a).tags || []) {
          const chip = document.createElement("span");
          chip.className = "tagchip";
          chip.textContent = tag;
          td.appendChild(chip);
        }
        if (td.childNodes.length === 0) td.textContent = "";
      } else if (action) {
        const btn = document.createElement("button");
        btn.className = "linkbtn";
        btn.textContent = action.label;
        btn.dataset.action = action.action;
        btn.dataset.value = action.value;
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
    const filtering = anyFilterActive();
    const clearBtn = $("btnEmptyClear");
    clearBtn.hidden = !(hasAny && filtering);
    clearBtn.onclick = clearResultFilters;
    const primary = $("btnEmptyPrimary");
    if (hasAny) {
      $("emptyTitle").textContent = "No papers match your filters";
      $("emptyDetail").textContent = "Filtering never removes stored results. Clear the filters to see all "
        + `${storedArticles.length} paper${storedArticles.length === 1 ? "" : "s"}.`;
      primary.textContent = "Clear filters";
      primary.onclick = clearResultFilters;
    } else {
      $("emptyTitle").textContent = "Build your paper library";
      $("emptyDetail").textContent = "Search Google Scholar or add papers by DOI to start screening.";
      primary.textContent = "Focus search";
      primary.onclick = () => $("query").focus();
    }
  }
  updateSortIndicators();
  applyDensity();
  updateYearHistogram();
  renderFilterChips();
  refreshViewCounts();
  refreshSummary();
  updateSelectionSurface();
  // Rendering reflects control availability; it never ends an operation.
  syncControls();
  refreshInspectorAvailability();
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

// 5.2: a real reset control instead of telling the user to change filters.
function clearResultFilters() {
  resultsQuery = "";
  $("resultsSearch").value = "";
  $("pdfFilter").value = "All";
  pdfFilter = "All";
  setDecisionFilter("all", { render: false });
  tagFilter = "";
  $("tagFilter").value = "";
  renderResults();
  scheduleSave();
}

/// Single writer for the decision filter, keeping the sidebar view and the
/// <select> in lockstep so an unknown stored value cannot hide every row.
function setDecisionFilter(value, options = {}) {
  const normalized = DECISION_VALUES.includes(value) ? value : "all";
  decisionFilter = normalized;
  currentView = normalized;
  const select = $("decisionFilter");
  if (select) select.value = normalized;
  if (options.render !== false) renderResults();
  if (options.persist !== false) scheduleSave();
}

function renderFilterChips() {
  const container = $("filterChips");
  if (!container) return;
  container.innerHTML = "";
  const chips = [];
  if (resultsQuery) chips.push({ label: `Text: ${resultsQuery}`, clear: () => { resultsQuery = ""; $("resultsSearch").value = ""; } });
  if (pdfFilter !== "All") chips.push({ label: `PDF: ${pdfFilter}`, clear: () => { pdfFilter = "All"; $("pdfFilter").value = "All"; } });
  if (decisionFilter !== "all") {
    const names = { undecided: "Undecided", include: "Included", exclude: "Excluded", maybe: "Maybe" };
    chips.push({ label: `Decision: ${names[decisionFilter] || decisionFilter}`, clear: () => setDecisionFilter("all", { render: false }) });
  }
  if (tagFilter.trim()) chips.push({ label: `Tag: ${tagFilter.trim()}`, clear: () => { tagFilter = ""; $("tagFilter").value = ""; } });
  container.hidden = chips.length === 0;
  for (const chip of chips) {
    const wrap = document.createElement("span");
    wrap.className = "chip";
    const text = document.createElement("span");
    text.textContent = chip.label;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Remove filter ${chip.label}`);
    remove.onclick = () => { chip.clear(); renderResults(); scheduleSave(); };
    wrap.appendChild(text);
    wrap.appendChild(remove);
    container.appendChild(wrap);
  }
  if (chips.length > 0) {
    const clearAll = document.createElement("button");
    clearAll.className = "ghost";
    clearAll.textContent = "Clear all";
    clearAll.onclick = clearResultFilters;
    container.appendChild(clearAll);
  }
}

function refreshSummary() {
  const rows = filteredArticles();
  const total = storedArticles.length;
  const filtering = anyFilterActive();
  let text;
  if (total === 0) {
    text = summaryBase;
  } else if (filtering) {
    text = `Showing ${rows.length} of ${total} papers`;
  } else {
    text = `${total} ${total === 1 ? "paper" : "papers"}`;
  }
  const provenance = displayedSearchRequest ? "" : (total > 0 ? " · Restored session" : "");
  const footer = $("summary");
  if (footer) footer.textContent = text + provenance;
  // The detailed, oracle-derived provenance lives in the status detail.
  $("appliedYear").textContent = displayedSearchRequest
    ? yearRangeLabel(displayedSearchRequest.yearFrom, displayedSearchRequest.yearTo)
    : (total > 0 ? "Restored session" : "Years: any time");
}

function onTableClick(e) {
  // Cell actions run first and never fall through to selection.
  const action = e.target.closest("[data-action]");
  if (action) {
    e.preventDefault();
    e.stopPropagation();
    const value = action.dataset.value || "";
    if (action.dataset.action === "reveal-pdf") invoke("reveal_path", { path: value }).catch(() => {});
    else if (action.dataset.action === "open-pdf" || action.dataset.action === "open-link") openUrl(value);
    return;
  }

  const box = e.target.closest("input.row-select");
  if (box) {
    const index = parseInt(box.dataset.index, 10);
    if (selection.has(index)) selection.delete(index);
    else selection.add(index);
    anchorRow = index;
    updateSelectionSurface();
    syncControls();
    return;
  }

  const tr = e.target.closest("tr[data-iid]");
  if (!tr) return;
  const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
  const modifier = e.ctrlKey || e.metaKey;

  if (e.shiftKey) {
    // Inclusive range in the visible sorted order (matching extendSelection).
    const ids = visibleIndices();
    const from = ids.indexOf(anchorRow === -1 ? index : anchorRow);
    const to = ids.indexOf(index);
    if (from !== -1 && to !== -1) {
      const [lo, hi] = from <= to ? [from, to] : [to, from];
      for (let k = lo; k <= hi; k++) selection.add(ids[k]);
    } else {
      selection.add(index);
    }
    focusRow = index;
    updateSelectionSurface();
    syncControls();
    return;
  }

  if (modifier) {
    if (selection.has(index)) selection.delete(index);
    else selection.add(index);
    anchorRow = index;
    updateSelectionSurface();
    syncControls();
    return;
  }

  selection.clear();
  selection.add(index);
  anchorRow = index;
  focusRow = index;
  updateSelectionSurface();
  syncControls();
  openDetailFor(index);
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
    ["Copy formatted citation", false, () => copyFormattedCitation(index)],
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

async function appendRankTrend(index, key, generation) {
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
  // Stale guard: the inspector may have switched papers while this ran.
  if (generation !== inspectorGeneration) return;
  const open = storedArticles[detailIndex];
  if (!open || reviewKey(open) !== key) return;
  const trend = history.map(([year, kind, rank]) => `${kind.toUpperCase()} ${rank}${year ? ` (${year})` : ""}`).join(" \u2192 ");
  const el = $("inspectorTrend");
  el.dataset.trend = trend;
  el.textContent = `Journal trend: ${trend}`;
}

// ---- record inspector ----
const INSPECTOR_FIELDS = ["detailDecision", "detailReason", "detailTags", "detailNote"];

function inspectorDraftFromInputs() {
  return {
    decision: $("detailDecision").value,
    reason: $("detailReason").value,
    tags: $("detailTags").value,
    note: $("detailNote").value,
  };
}

function draftIsDirty() {
  if (!reviewDraft || detailIndex < 0) return false;
  const current = inspectorDraftFromInputs();
  return current.decision !== reviewDraft.decision
    || current.reason !== reviewDraft.reason
    || current.tags !== reviewDraft.tags
    || current.note !== reviewDraft.note;
}

function markDraftState() {
  const dirty = draftIsDirty();
  const flag = $("inspectorDirty");
  if (flag) flag.hidden = !dirty;
}

/// Ask what to do with unsaved inspector edits. Cancel/Escape => "keep".
function resolveReviewDraft() {
  return new Promise((resolve) => {
    const dialog = $("unsavedReviewDialog");
    if (!draftIsDirty()) return resolve("discard");
    $("unsavedDetail").textContent = "You have review edits that have not been saved for this paper.";
    const done = (value) => {
      dialog.close();
      resolve(value);
    };
    $("btnUnsavedKeep").onclick = () => done("keep");
    $("btnUnsavedDiscard").onclick = () => done("discard");
    $("btnUnsavedSave").onclick = () => done("save");
    dialog.addEventListener("cancel", () => resolve("keep"), { once: true });
    dialog.showModal();
    $("btnUnsavedKeep").focus();
  });
}

function updateInspectorMeta(article) {
  const meta = $("inspectorMeta");
  if (!meta) return;
  meta.innerHTML = "";
  const none = "unknown";
  const year = article.publicationYear ?? article.publication_year;
  const rows = [
    ["Journal", article.journal || none],
    ["Year", year == null ? none : String(year)],
    ["DOI", article.doi || none],
    ["Index", indexingLabel(article) || none],
    ["SJR", (article.sjrQuartile ?? article.sjr_quartile) || none],
    ["SINTA", (article.sintaRank ?? article.sinta_rank) || none],
    ["Cited by", (article.citedBy ?? article.cited_by) == null ? none : String(article.citedBy ?? article.cited_by)],
    ["OA", (article.oaStatus ?? article.oa_status) || none],
    ["PDF", (article.pdfStatus ?? article.pdf_status) || none],
  ];
  for (const [label, value] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = label;
    const dd = document.createElement("dd");
    dd.textContent = value;
    meta.appendChild(dt);
    meta.appendChild(dd);
  }
}

function updateInspectorBadges(article) {
  const container = $("inspectorBadges");
  if (!container) return;
  container.innerHTML = "";
  const decision = decisionLabel(article);
  if (decision) {
    const badge = document.createElement("span");
    badge.className = `badge ${decision}`;
    badge.textContent = decision.toUpperCase();
    container.appendChild(badge);
  }
  const notice = (article.retraction ?? "").trim();
  if (notice) {
    const badge = document.createElement("span");
    badge.className = "badge retracted";
    badge.textContent = notice === "retraction" ? "RETRACTED" : notice.toUpperCase();
    container.appendChild(badge);
  }
}

/// id = record key, so late async completions cannot decorate another paper.
async function openDetailFor(index, options = {}) {
  const article = storedArticles[index];
  if (!article) return false;
  if (options.resolveDraft !== false && detailIndex !== index) {
    const choice = await resolveReviewDraft();
    if (choice === "keep") return false;
    if (choice === "save") {
      const saved = await saveDetailReview({ close: false });
      if (!saved) return false;
    }
  }
  detailIndex = index;
  reviewDraft = null;
  const key = reviewKey(article);
  const entry = reviewEntry(article);
  const generation = ++inspectorGeneration;

  $("inspectorTitle").textContent = article.title || "(untitled)";
  $("inspectorAuthors").textContent = article.authors || "Authors unknown";
  updateInspectorBadges(article);
  updateInspectorMeta(article);

  $("detailDecision").value = entry.decision || "";
  $("detailReason").value = entry.reason || "";
  $("detailTags").value = (entry.tags || []).join(", ");
  $("detailNote").value = entry.note || "";
  reviewDraft = inspectorDraftFromInputs();

  const localPath = article.pdfLocalPath ?? article.pdf_local_path ?? "";
  const pdfUrl = article.pdfUrl ?? article.pdf_url ?? "";
  $("btnDetailOpenLink").disabled = !(article.link || "");
  $("btnDetailOpenPdf").disabled = !(localPath || pdfUrl);
  $("btnDetailCopyDoi").disabled = !(article.doi || "");
  $("btnDetailCopyTitle").disabled = !(article.title || "");
  $("menuCiteForward").disabled = !(article.doi || "").trim();
  $("menuCiteBackward").disabled = !(article.doi || "").trim();
  $("detailText").textContent = articleDetailText(article);
  $("inspectorAbstractText").hidden = true;
  $("inspectorAbstractText").textContent = "";
  $("inspectorTrend").textContent = "";
  $("detailReasonField").hidden = ($("detailDecision").value !== "exclude");
  updateTagPreview();
  markDraftState();

  const inspector = $("recordInspector");
  inspector.hidden = false;
  document.body.classList.add("inspector-open");
  updateInspectorMode();
  $("inspectorScrim").hidden = !document.body.classList.contains("inspector-modal");
  if (document.body.classList.contains("inspector-modal")) {
    $("btnInspectorClose").focus();
  }
  refreshInspectorAvailability();
  appendRankTrend(index, key, generation);
  return true;
}

/// Keep Tab inside the inspector while it is a modal overlay.
function trapInspectorFocus(e) {
  if (e.key !== "Tab" || !document.body.classList.contains("inspector-modal")) return;
  const focusable = $("recordInspector").querySelectorAll(
    "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])"
  );
  if (focusable.length === 0) return;
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}

function updateInspectorMode() {
  const modal = window.innerWidth < 1280;
  document.body.classList.toggle("inspector-modal", modal && !$("recordInspector").hidden);
  const main = document.querySelector(".sidebar");
  const workspace = document.querySelector(".workspace");
  if ($("recordInspector").hidden) {
    document.body.classList.remove("inspector-modal");
    if (main) main.removeAttribute("inert");
    if (workspace) workspace.removeAttribute("inert");
  } else if (document.body.classList.contains("inspector-modal")) {
    if (main) main.setAttribute("inert", "");
    if (workspace) workspace.setAttribute("inert", "");
  } else {
    if (main) main.removeAttribute("inert");
    if (workspace) workspace.removeAttribute("inert");
  }
  $("inspectorScrim").hidden = !document.body.classList.contains("inspector-modal");
}

/// Move through the visible order; the draft guard decides whether it happens.
async function stepInspector(delta) {
  const ids = visibleIndices();
  const position = ids.indexOf(detailIndex);
  if (position === -1) return;
  const next = position + delta;
  if (next < 0 || next >= ids.length) return;
  await openDetailFor(ids[next]);
}

function updateTagPreview() {
  const preview = $("detailTagPreview");
  if (!preview) return;
  const tags = $("detailTags").value.split(",").map((t) => t.trim()).filter(Boolean);
  preview.textContent = tags.length ? tags.join(" · ") : "Comma-separated; shown as chips in the table.";
}

function refreshInspectorAvailability() {
  const inspector = $("recordInspector");
  if (!inspector || inspector.hidden) return;
  const visible = visibleIndices();
  const position = detailIndex >= 0 ? visible.indexOf(detailIndex) : -1;
  $("btnInspectorPrev").disabled = position <= 0;
  $("btnInspectorNext").disabled = position === -1 || position >= visible.length - 1;
  const trend = $("inspectorTrend");
  if (position === -1 && trend && !trend.dataset.trend) {
    trend.textContent = "This paper is not in the current view; adjust filters to navigate.";
  }
}

/// Re-read the inspected record after a collection update without discarding
/// an in-progress draft when the record itself did not change.
function refreshInspector() {
  const inspector = $("recordInspector");
  if (!inspector || inspector.hidden) return;
  const article = storedArticles[detailIndex];
  if (!article) return;
  $("inspectorTitle").textContent = article.title || "(untitled)";
  $("inspectorAuthors").textContent = article.authors || "Authors unknown";
  updateInspectorBadges(article);
  updateInspectorMeta(article);
  $("detailText").textContent = articleDetailText(article);
  refreshInspectorAvailability();
}

function closeInspector(options = {}) {
  const inspector = $("recordInspector");
  if (!inspector) return;
  inspector.hidden = true;
  document.body.classList.remove("inspector-open");
  updateInspectorMode();
  inspectorGeneration += 1;
  reviewDraft = null;
  if (!options.keepIndex) detailIndex = -1;
}

async function requestInspectorClose() {
  const choice = await resolveReviewDraft();
  if (choice === "keep") return false;
  if (choice === "save") {
    const saved = await saveDetailReview({ close: false });
    if (!saved) return false;
  }
  const restoreFocus = focusRow >= 0 ? focusRow : detailIndex;
  closeInspector();
  if (restoreFocus >= 0) {
    const row = document.querySelector(`#tableBody tr[data-iid="article-${restoreFocus}"]`);
    if (row) row.focus();
  }
  return true;
}

async function openAbstract(index) {
  const article = storedArticles[index];
  if (!article) return;
  const key = reviewKey(article);
  const generation = inspectorGeneration;
  const target = $("inspectorAbstractText");
  target.hidden = false;
  target.textContent = "Fetching abstract…";
  let text = null;
  let failed = false;
  try {
    text = await invoke("fetch_abstract", { article });
  } catch {
    failed = true;
  }
  // Stale guard: the inspector may have moved on while the request ran.
  if (generation !== inspectorGeneration || reviewKey(storedArticles[detailIndex] || {}) !== key) return;
  if (text) {
    target.textContent = text;
  } else if (failed) {
    target.textContent = "The abstract provider could not be reached. Try again, or open the paper at its source.";
  } else {
    target.textContent = "No abstract is available from Crossref or OpenAlex for this record. Open the link to read the paper at its source.";
  }
}

/// Returns true only when the project write actually succeeded.
async function saveDetailReview(options = {}) {
  if (detailIndex === -1) return false;
  const article = storedArticles[detailIndex];
  if (!article) return false;
  const decision = $("detailDecision").value;
  const reason = $("detailReason").value.trim();
  const note = $("detailNote").value.trim();
  const tags = $("detailTags").value.split(",").map((t) => t.trim()).filter(Boolean);
  updateReview([detailIndex], (entry, now) => {
    entry.decision = decision;
    entry.reason = decision === "exclude" ? reason : "";
    entry.note = note;
    entry.tags = tags;
    entry.updated_at = now;
  });
  const ok = await flushReviewSave();
  if (!ok) {
    setStatus("Review not saved", "The changes stay in this window; retry Save review.", "error");
    return false;
  }
  reviewDraft = inspectorDraftFromInputs();
  markDraftState();
  if (options.close !== false) {
    closeInspector();
  }
  setStatus("Review saved", "The decision, note and tags were saved for this record.", "success");
  return true;
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

// ---- details rendering ----
async function openDetail() {
  if (selection.size !== 1) {
    setStatus("Select one paper", "Select a single row to inspect it, or use the row's checkbox.", "warning");
    return;
  }
  await openDetailFor([...selection][0]);
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
    const status = r ? (r.status ?? r.Status) : "";
    if (!r || !status) {
      // Malformed worker payloads must not erase a working library.
      setStatus("Search failed", "The application did not receive a valid result. Stored papers were kept.", "error");
      return;
    }
    const articles = Array.isArray(r.articles) ? r.articles : [];
    const emptyPartial = ["CANCELLED", "RATE_LIMITED", "BLOCKED", "NETWORK_ERROR", "PARSING_ERROR"].includes(status)
      && articles.length === 0;
    const requested = r.requestedPages ?? r.requested_pages ?? 0;
    const successful = r.successfulPages ?? r.successful_pages ?? 0;
    const duplicates = r.duplicatesRemoved ?? r.duplicates_removed ?? 0;
    const invalid = r.invalidArticlesRemoved ?? r.invalid_articles_removed ?? 0;
    // result_summary() L249-261: singular/plural, not "(s)".
    const resultWord = articles.length === 1 ? "result" : "results";
    // Python appends the plural "s" to "removed" (L255/L259) — reproduced verbatim.
    const byDoi = r.duplicatesByDoi ?? r.duplicates_by_doi ?? 0;
    summaryBase = `${articles.length} ${resultWord} found | ${successful}/${requested} pages completed` +
      (duplicates ? ` | ${duplicates} duplicate removed${duplicates === 1 ? "" : "s"}` : "") +
      (byDoi ? ` (${byDoi} matched by DOI)` : "") +
      (invalid ? ` | ${invalid} invalid record removed${invalid === 1 ? "" : "s"}` : "");
    setProgress(successful, requested);
    let message = r.message || STATUS_DEFAULTS[status] || "";
    // status_message() L203-206: the hint rides along with the message.
    if (status === "SUCCESS") message += CHECK_METADATA_HINT;
    setStatus(STATUS_HEADINGS[status] || "Ready", message, STATUS_TONES[status] || "ready");
    // Retry only for transport/provider failures; a clean "no results" is not a
    // failure to retry (STATUS_TONES marks it "error" for emphasis only).
    const retryable = status === "RATE_LIMITED" || status === "NETWORK_ERROR"
      || status === "PARSING_ERROR" || status === "BLOCKED";
    retryableResult = retryable;
    if (!retryable) {
      recordQuery(lastRequest?.query || "");
      toggleConfig(true); // search succeeded: give the table the space
    }
    if (emptyPartial) {
      // A failed attempt that collected nothing leaves the working set intact.
      applyArticleCollection(storedArticles, "update", { persist: false, summary: summaryBase });
    } else {
      displayedSearchRequest = lastRequest ? { ...lastRequest } : displayedSearchRequest;
      applyArticleCollection(articles, "replace", { summary: summaryBase });
    }
    refreshViewCounts();
    renderResults();
    scheduleSave();
    return;
  }
  if (msg.kind === "task_result") {
    finishOperation();
    const op = msg.operation;
    const p = msg.payload;
    if (op === "check") {
      const articles = p.articles ?? [];
      const sameShape = Array.isArray(articles) && articles.length === storedArticles.length;
      if (sameShape) applyArticleCollection(articles, "update", { persist: false });
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
      refreshViewCounts();
      renderResults();
      saveResultsSoon();
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
        applyArticleCollection(p.articles, "update", { persist: false });
      }
      // _finish_import (L1420-1428): filename, cumulative total, and the
      // gained-label sentence always (even "0 displayed record(s)").
      const fileName = String(p.path || "").split(/[\\/]/).pop();
      const yearNote = year
        ? ` Ranking year ${year} was used for records without one.`
        : " Records without a year stay unknown.";
      refreshViewCounts();
      setStatus("Rankings imported",
        `${p.count} ranking records added from ${fileName}; ${p.total} record(s) loaded in total (earlier imports were kept).` +
        yearNote +
        ` ${gained} displayed record(s) gained a journal label.`,
        "success");
      renderResults();
      saveResultsSoon();
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
      refreshViewCounts();
      renderResults();
      saveResultsSoon();
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
  showReport(
    "Journals with no imported ranking match",
    gaps.size === 0
      ? "Every stored record already carries a rank label."
      : `${gaps.size} distinct journal(s) across ${storedArticles.length} records have no SJR/SINTA match:\n\n${lines.join("\n")}`
  );
}

// ---- theme (5.11) ----
function applyTheme(theme) {
  const chosen = theme || "system";
  if (chosen === "system") {
    const dark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  } else {
    document.documentElement.dataset.theme = chosen;
  }
  if ($("themeSelect")) $("themeSelect").value = chosen;
}

// ---- review project (P3) ----
// Decisions/notes/tags are keyed by DOI (or normalized title) rather than row
// index, so they follow a paper across re-searches and appends.
let reviewProject = { version: 1, entries: {}, baselines: {} };

function reviewKey(a) {
  const doi = String(a.doi || "").trim().toLowerCase();
  if (doi) return "doi:" + doi;
  const title = String(a.title || "").toLowerCase().replace(/[^a-z0-9]+/g, "");
  return title ? "title:" + title : "";
}

function reviewEntry(a) {
  const key = reviewKey(a);
  return (key && reviewProject.entries[key]) || { decision: "", reason: "", note: "", tags: [] };
}

function decisionLabel(a) {
  const d = reviewEntry(a).decision;
  return d ? d : "";
}

function tagsLabel(a) {
  const t = reviewEntry(a).tags || [];
  return t.join(", ");
}

async function loadReviewProject() {
  try {
    const stored = await invoke("load_review_project");
    if (stored && stored.entries) reviewProject = stored;
  } catch {
    /* first run: empty project */
  }
}

function scheduleReviewSave() {
  clearTimeout(reviewSaveTimer);
  reviewSaveTimer = setTimeout(() => {
    flushReviewSave().catch(() => {});
  }, 400);
}
let reviewSaveTimer = null;
let reviewSaveQueue = Promise.resolve();

/// Persist the current review project and RESOLVE only when disk accepted it.
/// Queued writes are serialized so an older snapshot can never land last.
function flushReviewSave() {
  clearTimeout(reviewSaveTimer);
  reviewSaveTimer = null;
  const snapshot = JSON.parse(JSON.stringify(reviewProject));
  reviewSaveQueue = reviewSaveQueue.then(async () => {
    await invoke("save_review_project", { project: snapshot });
    return true;
  }).catch(() => false);
  return reviewSaveQueue;
}

/// In-app replacement for window.prompt, which a Tauri webview can suppress.
function askText(title, initial) {
  return new Promise((resolve) => {
    $("promptTitle").textContent = title;
    $("promptInput").value = initial || "";
    const dialog = $("promptDialog");
    const done = (value) => {
      $("btnPromptOk").onclick = null;
      $("btnPromptCancel").onclick = null;
      if (dialog.open) dialog.close();
      resolve(value);
    };
    $("btnPromptOk").onclick = () => done($("promptInput").value.trim());
    $("btnPromptCancel").onclick = () => done("");
    dialog.showModal();
    $("promptInput").focus();
  });
}

/// Apply one field change to every selected row (or the detail row).
function updateReview(indices, mutate) {
  const now = Math.floor(Date.now() / 1000);
  for (const index of indices) {
    const a = storedArticles[index];
    if (!a) continue;
    const key = reviewKey(a);
    if (!key) continue;
    const entry = reviewProject.entries[key] || { decision: "", reason: "", note: "", tags: [], updated_at: 0, found_by: "" };
    mutate(entry, now);
    entry.updated_at = now;
    reviewProject.entries[key] = entry;
  }
  scheduleReviewSave();
  renderResults();
}

function selectedIndices() {
  if (selection.size > 0) return [...selection];
  return [];
}

async function setDecision(decision) {
  if (running) return;
  const indices = selectedIndices();
  if (indices.length === 0) {
    setStatus("No papers selected", "Select one or more rows, then record a decision.", "warning");
    return;
  }
  // Bulk application would overwrite inspector fields, so settle that draft first.
  const choice = await resolveReviewDraft();
  if (choice === "keep") return;
  if (choice === "save") {
    const saved = await saveDetailReview({ close: false });
    if (!saved) return;
  }
  const snapshot = indices.slice();
  updateReview(snapshot, (entry) => {
    entry.decision = decision;
    if (decision === "exclude") {
      // Preserve an existing coded reason; a bulk decision cannot invent one.
      if (entry.reason && entry.decision === "exclude") return;
      entry.reason = "";
      entry.decision = decision;
    } else {
      entry.reason = "";
      entry.decision = decision;
    }
  });
  const ok = await flushReviewSave();
  if (!ok) {
    setStatus("Review not saved", "The decision is shown but could not be written to disk.", "error");
    return;
  }
  setStatus("Review saved", `${snapshot.length} record(s) marked ${decision}.`, "success");
}

// ---- extra exports (P2) ----
const EXTRA_FORMATS = {
  json: "scholar_articles.json",
  markdown: "scholar_articles.md",
  tsv: "scholar_articles.tsv",
  enw: "scholar_articles.enw",
  "reading-list": "scholar_reading_list.html",
  journals: "scholar_journals.csv",
  details: "scholar_details.txt",
};
const EXTRA_CITATIONS = {
  "csl-json": "scholar_citations.json",
  "bibtex-stable": "scholar_citations.bib",
  "bibtex-files": "scholar_citations_files.bib",
  grouped: "scholar_bibliography.md",
};

/// The single scope resolver. Nothing may silently widen or narrow a scope.
function exportRecords(scope = $("exportScope").value || "visible") {
  if (scope === "selected") return [...selection].map((i) => storedArticles[i]).filter(Boolean);
  if (scope === "all") return storedArticles.slice();
  return visibleArticles().map(([, a]) => a);
}

function exportFolder() {
  // The destination is the visible, user-controlled folder field; a search
  // request snapshot must never override what the dialog shows.
  return $("folder").value.trim();
}

function selectedColumns() {
  // Hidden columns are omitted from the export, matching what is on screen.
  const labels = {
    title: "Title", authors: "Authors", link: "Link", index: "Relevance Score",
    rank: "SJR Quartile", pdf: "PDF Status",
  };
  return Object.keys(labels)
    .filter((key) => !hiddenColumns.has(key))
    .map((key) => labels[key]);
}

async function extraExport(format, records = null) {
  if (!Array.isArray(records)) records = exportRecords();
  if (records.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const path = joinPath(exportFolder(), EXTRA_FORMATS[format]);
  try {
    await invoke("export_tabular", { articles: records, path, format, columns: selectedColumns() });
    setStatus("Ready", `Exported ${records.length} result(s) to ${path}.`, "ready");
    rememberExportFolder(path);
  } catch (err) {
    const msg = errMessage(err).replace("Export failed: ", "");
    setStatus("Ready", `Export failed: ${msg}`, "error");
    showError("Export failed", msg);
  }
}

async function extraCitationExport(format, records = null) {
  if (!Array.isArray(records)) records = exportRecords();
  if (records.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const path = joinPath(exportFolder(), EXTRA_CITATIONS[format]);
  try {
    await invoke("export_citations_extra", {
      articles: records,
      path,
      format,
      style: $("citeStyle").value,
      groupBy: $("groupBy").value || null,
    });
    setStatus("Ready", `Exported ${records.length} citation(s) to ${path}.`, "ready");
    rememberExportFolder(path);
  } catch (err) {
    const msg = errMessage(err).replace("Export failed: ", "");
    setStatus("Ready", `Export failed: ${msg}`, "error");
    showError("Export failed", msg);
  }
}

async function richExcelExport(records = null) {
  if (!Array.isArray(records)) records = exportRecords();
  if (records.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const group = $("groupBy").value || null;
  const path = joinPath(exportFolder(), group ? `scholar_articles_by_${group}.xlsx` : "scholar_articles.xlsx");
  try {
    await invoke("export_excel_rich", { articles: records, path, columns: selectedColumns(), groupBy: group });
    setStatus("Ready", `Exported ${records.length} result(s) to ${path}.`, "ready");
    rememberExportFolder(path);
  } catch (err) {
    const msg = errMessage(err).replace("Export failed: ", "");
    setStatus("Ready", `Export failed: ${msg}`, "error");
    showError("Export failed", msg);
  }
}

/// Numbered Markdown bibliography of the current export scope, on the clipboard.
async function copyBibliography(records = null) {
  if (!Array.isArray(records)) records = exportRecords();
  if (records.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const tmp = joinPath(exportFolder() || ".", "scholar_bibliography.md");
  try {
    await invoke("export_citations_extra", {
      articles: records,
      path: tmp,
      format: "grouped",
      style: $("citeStyle").value,
      groupBy: $("groupBy").value || "year",
    });
    const text = await invoke("read_text_file", { path: tmp }).catch(() => null);
    if (typeof text === "string" && text) {
      copyText(text);
      setStatus("Copied", `Copied ${records.length} reference(s) as ${$("citeStyle").value.toUpperCase()}.`, "success");
    } else {
      setStatus("Copied", `Wrote the bibliography to ${tmp}.`, "success");
    }
  } catch (err) {
    showError("Copy failed", errMessage(err));
  }
}

async function copyFormattedCitation(index) {
  const article = storedArticles[index];
  if (!article) return;
  try {
    const text = await invoke("format_citation_text", { article, style: $("citeStyle").value });
    if (text) {
      copyText(text);
      setStatus("Copied", `Copied the ${$("citeStyle").value.toUpperCase()} reference for this record.`, "success");
    }
  } catch (err) {
    showError("Copy failed", errMessage(err));
  }
}

// ---- review actions ----
async function showPrisma() {
  try {
    const summary = await invoke("review_prisma", { articles: storedArticles });
    showReport("PRISMA counts", String(summary));
  } catch (err) {
    showError("PRISMA counts", errMessage(err));
  }
}

function showReport(title, text) {
  $("reportTitle").textContent = title;
  $("reportText").textContent = text;
  const dialog = $("reportDialog");
  if (!dialog.open) dialog.showModal();
}

async function exportScreeningLog() {
  const rows = await invoke("review_screening_log", { articles: storedArticles }).catch(() => []);
  if (!Array.isArray(rows) || rows.length === 0) {
    setStatus("Nothing to export", "Record at least one decision or note first.", "warning");
    return;
  }
  const path = joinPath(exportFolder() || ".", "screening_log.csv");
  const header = "Title,DOI,Decision,Reason,Tags,Note,Found by,Updated (unix),SJR,SINTA\r\n";
  const body = rows.map((r) => [r.title, r.doi, r.decision, r.reason, r.tags, r.note, r.found_by, r.updated_at, r.sjr_quartile, r.sinta_rank]
    .map((v) => {
      const s = String(v ?? "");
      return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
    }).join(",")).join("\r\n");
  try {
    await invoke("write_text_file", { path, content: header + body + "\r\n" });
    setStatus("Ready", `Screening log written to ${path}.`, "ready");
    rememberExportFolder(path);
  } catch (err) {
    showError("Export failed", errMessage(err));
  }
}

async function showNearDuplicates() {
  const pairs = await invoke("review_near_duplicates", { articles: storedArticles }).catch(() => []);
  if (!Array.isArray(pairs) || pairs.length === 0) {
    showReport("Near-duplicate report", "No likely duplicate pairs above 0.8 similarity.");
    return;
  }
  const lines = pairs.slice(0, 40).map((p) => {
    const a = storedArticles[p.first] || {};
    const b = storedArticles[p.second] || {};
    return `${(p.similarity * 100).toFixed(0)}%  ${a.title || "?"}\n     vs ${b.title || "?"}`;
  });
  showReport("Near-duplicate report", `${pairs.length} pair(s) above 0.8 similarity:\n\n${lines.join("\n\n")}`);
}

async function saveBaseline() {
  if (running) return;
  await flushReviewSave();
  const name = await askText("Baseline name", "week1");
  if (!name) return;
  try {
    await invoke("review_save_baseline", { name, articles: storedArticles });
    // Reload: a later note save must not overwrite the newly stored baseline.
    await loadReviewProject();
    setStatus("Baseline saved", `Saved ${storedArticles.length} record(s) as "${name}".`, "success");
  } catch (err) {
    showError("Could not save baseline", errMessage(err));
  }
}

async function diffBaseline() {
  if (running) return;
  await flushReviewSave();
  const name = await askText("Baseline name to compare against", "week1");
  if (!name) return;
  const diff = await invoke("review_diff_baseline", { name, articles: storedArticles }).catch(() => null);
  if (!diff) {
    showError("Baseline diff", `No baseline named "${name}".`);
    return;
  }
  const added = (diff.added || []).map((i) => storedArticles[i]?.title || "?").slice(0, 20);
  showReport(
    "Baseline diff",
    `${diff.added.length} new · ${diff.removed} no longer present · ${diff.unchanged} unchanged` +
      (added.length ? `\n\nNew records:\n${added.map((t) => `· ${t}`).join("\n")}` : "")
  );
}

async function rescoreWithQuery() {
  if (running) return;
  if (storedArticles.length === 0) {
    setStatus("Nothing to score", "Collect some papers before re-scoring.", "warning");
    return;
  }
  const query = await askText("Re-score against query", $("query").value.trim());
  if (!query) return;
  setRunning("review");
  try {
    const ranked = await invoke("review_rescore", {
      articles: storedArticles,
      query,
      enabled: $("rankingEnabled").checked,
    });
    applyArticleCollection(ranked, "reorder");
    setStatus("Re-scored", `Re-ranked ${storedArticles.length} record(s) against "${query}".`, "success");
  } catch (err) {
    showError("Re-score failed", errMessage(err));
  } finally {
    setIdleState();
  }
}

async function importOwnExport() {
  if (running) return;
  const path = await dialog("open", "Choose a CSV or JSON export to import");
  if (!path) return;
  const before = storedArticles.length;
  setRunning("review");
  try {
    const merged = await invoke("review_import_export", { path, current: storedArticles });
    applyArticleCollection(merged, "append");
    const added = Math.max(0, storedArticles.length - before);
    setStatus("Imported", `${added} new record(s) merged from ${path}.`, "success");
  } catch (err) {
    showError("Import failed", errMessage(err));
  } finally {
    setIdleState();
  }
}

// ---- discovery (P7) ----
function openPasteDois() {
  if (running) return;
  $("pasteDois").value = "";
  $("pasteDialog").showModal();
  $("pasteDois").focus();
}

async function addPastedDois() {
  const text = $("pasteDois").value;
  $("pasteDialog").close();
  try {
    const before = storedArticles.length;
    setRunning("review");
    const merged = await invoke("discovery_add_dois", { text, current: storedArticles });
    applyArticleCollection(merged, "append");
    const added = Math.max(0, storedArticles.length - before);
    setStatus("Records added", `${added} new record(s) resolved from pasted DOIs.`, "success");
  } catch (err) {
    showError("Could not add records", errMessage(err));
  } finally {
    setIdleState();
  }
}

async function citationChase(direction) {
  const index = selection.size > 0 ? [...selection][0] : -1;
  const article = index >= 0 ? storedArticles[index] : null;
  const doi = article && article.doi ? article.doi.trim() : "";
  if (!doi) {
    setStatus("No DOI", "Select a record that has a DOI, then chase its citations.", "warning");
    return;
  }
  setStatus("Chasing citations", `Looking up ${direction === "citations" ? "works citing" : "works cited by"} ${doi}.`, "running");
  try {
    const found = await invoke("discovery_citation_chase", { doi, direction, limit: 25 });
    if (!Array.isArray(found) || found.length === 0) {
      setStatus("Nothing found", "OpenAlex returned no related works for this DOI.", "warning");
      return;
    }
    // Offer the discovery as an import rather than replacing the current set.
    const before = storedArticles.length;
    setRunning("review");
    const merged = await invoke("review_append", { articles: found, current: storedArticles });
    applyArticleCollection(merged, "append");
    const added = Math.max(0, storedArticles.length - before);
    setStatus("Citations chased", `Added ${added} record(s) from ${direction}.`, "success");
  } catch (err) {
    showError("Citation chasing failed", errMessage(err));
  } finally {
    setIdleState();
  }
}

// ---- saved filter expressions (2.8) ----
function currentFilter() {
  return {
    pdf: $("pdfFilter").value || "All",
    decision: decisionFilter,
    tag: tagFilter,
    query: resultsQuery,
  };
}

function applyFilter(filter) {
  if (!filter) return;
  const pdf = PDF_FILTERS.includes(filter.pdf) ? filter.pdf : "All";
  $("pdfFilter").value = pdf;
  pdfFilter = pdf;
  setDecisionFilter(filter.decision || "all", { persist: false, render: false });
  tagFilter = filter.tag || "";
  $("tagFilter").value = tagFilter;
  resultsQuery = filter.query || "";
  $("resultsSearch").value = resultsQuery;
  renderResults();
  scheduleSave();
}

/// Tell the user why the library looks filtered after a restore instead of an
/// unexplained short list.
function showFiltersPopoverHint() {
  const chips = $("filterChips");
  if (chips && !chips.hidden) return; // chips already explain the view
  setStatus("Filters restored", "A saved decision filter is active. Use Clear all to see every paper.", "warning");
}

function renderSavedFilters() {
  const select = $("savedFilters");
  select.innerHTML = '<option value="">saved filters…</option>';
  for (const name of Object.keys(savedFilters).sort()) {
    const option = document.createElement("option");
    option.value = name;
    option.textContent = name;
    select.appendChild(option);
  }
}

async function saveCurrentFilter() {
  const name = await askText("Save this filter as", "");
  if (!name) return;
  savedFilters[name] = currentFilter();
  renderSavedFilters();
  scheduleSave();
  setStatus("Filter saved", `Saved "${name}".`, "success");
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
  if (!panel) return;
  // toggleConfig() flips; toggleConfig(true) collapses; toggleConfig(false) expands.
  const collapsed = collapse === undefined ? !panel.hidden : !!collapse;
  panel.hidden = collapsed;
  $("btnToggleConfig").setAttribute("aria-expanded", collapsed ? "false" : "true");
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
    theme: $("themeSelect") ? $("themeSelect").value : "system",
    decision_filter: decisionFilter,
    tag_filter: tagFilter,
    saved_filters: savedFilters,
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
  applyTheme(saved.theme || "system");
  setDecisionFilter(saved.decision_filter || "all", { persist: false, render: false });
  tagFilter = saved.tag_filter || "";
  if ($("tagFilter")) $("tagFilter").value = tagFilter;
  savedFilters = saved.saved_filters || {};
  renderSavedFilters();
  // 7.1: restore the window's size and position from the last session.
  invoke("restore_window_geometry").catch(() => {});
  await loadReviewProject();
  onYearPresetChanged();
  renderResults();
  await restoreResults();
  resultsReady = true;
  // Re-render now that review entries exist, so badges/counts are truthful.
  renderResults();
  if (saved.decision_filter && saved.decision_filter !== "all") showFiltersPopoverHint();
}

async function restoreResults() {
  if (running || storedArticles.length > 0) return;
  let articles = [];
  try { articles = await invoke("load_results"); } catch { return; }
  if (!Array.isArray(articles) || articles.length === 0) return;
  const word = articles.length === 1 ? "result" : "results";
  displayedSearchRequest = null;
  summaryBase = `${articles.length} ${word} restored from the last session`;
  applyArticleCollection(articles, "replace", { summary: summaryBase, persist: false });
  // A persisted decision/tag filter can hide every restored row; say why rather
  // than showing an empty table that looks like a failed restore.
  const shown = document.querySelectorAll("#tableBody tr").length;
  if (shown === 0) {
    setStatus(
      "Restored, but filtered out",
      `${articles.length} ${word} restored, none match the active filter. Use Clear filters.`,
      "warning",
    );
  }
}

let resultsTimer = null;
let resultsReady = false; // no write before the first restore completes
function saveResultsSoon() {
  if (!resultsReady) return;
  clearTimeout(resultsTimer);
  resultsTimer = setTimeout(() => {
    // An accepted empty collection IS the current state; skipping it would
    // resurrect old papers on the next launch.
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
