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

const STATUS_DEFAULTS = {
  SUCCESS: "Run 'Check metadata / PDFs' to look up journal details and public PDFs.",
  PARTIAL_SUCCESS: "Some pages could not be retrieved; collected results are kept.",
  NO_RESULTS: "Try different keywords or fewer filters.",
  RATE_LIMITED: "Google Scholar is throttling requests. Wait a moment, then retry.",
  BLOCKED: "Google Scholar is showing a challenge page. Try again later.",
  NETWORK_ERROR: "Google Scholar could not be reached. Check the connection.",
  PARSING_ERROR: "The results page could not be read; Google Scholar may have changed.",
  CANCELLED: "The search was cancelled.",
};
const STATUS_HEADINGS = {
  SUCCESS: "Search complete",
  PARTIAL_SUCCESS: "Search partially completed",
  NO_RESULTS: "No results",
  RATE_LIMITED: "Rate limited",
  BLOCKED: "Blocked by Google Scholar",
  NETWORK_ERROR: "Search failed",
  PARSING_ERROR: "Search failed",
  CANCELLED: "Search cancelled",
};
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

const $ = (id) => document.getElementById(id);

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
  $("btnCheck").onclick = checkMetadataPdfs;
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
  $("yearPreset").onchange = onYearPresetChanged;
  $("pdfFilter").onchange = renderResults;
  $("tableBody").onclick = onTableClick;
  $("tableBody").ondblclick = onTableDblClick;
  document.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !running && document.activeElement === $("query")) startSearch();
    if (e.key === "Escape" && running) cancelOperation();
  });
  setIdleState();
}

function onYearPresetChanged() {
  const idx = $("yearPreset").selectedIndex;
  const custom = idx === 4;
  $("yearFrom").disabled = !custom || running;
  $("yearTo").disabled = !custom || running;
  updateAppliedYear();
  renderResults();
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
  for (const id of ["btnSearch", "btnCheck", "btnDownload", "btnDetails", "btnImport", "btnBrowse", "btnExportExcel", "btnExportCsv", "btnExportCitations"]) {
    $(id).disabled = true;
  }
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
  running = false;
  currentOperation = null;
  const hasArticles = storedArticles.length > 0;
  const hasVisible = visibleArticles().length > 0;
  $("btnSearch").disabled = false;
  $("btnCancel").disabled = true;
  $("btnCheck").disabled = !hasArticles;
  $("btnDownload").disabled = !hasArticles;
  $("btnDetails").disabled = !hasArticles;
  $("btnImport").disabled = false;
  $("btnBrowse").disabled = false;
  $("btnExportExcel").disabled = !hasVisible;
  $("btnExportCsv").disabled = !hasVisible;
  $("btnExportCitations").disabled = !hasVisible;
  $("query").disabled = false;
  $("pages").disabled = false;
  $("folder").disabled = false;
  $("rankingPath").disabled = false;
  $("rankingYear").disabled = false;
  $("rankingEnabled").disabled = false;
  $("pdfFilter").disabled = false;
  $("yearPreset").disabled = false;
  onYearPresetChanged();
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
  const { from, to } = currentYearBounds();
  try {
    await invoke("search_scholar", {
      query: $("query").value,
      pages: parseInt($("pages").value, 10) || 1,
      rankingEnabled: $("rankingEnabled").checked,
      outputFolder: $("folder").value,
      yearFrom: from,
      yearTo: to,
    });
  } catch (err) {
    // Validation errors: inline status, no dialog (matches Python).
    setStatus("Check search setup", String(err), "error");
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
async function checkMetadataPdfs() {
  if (running || storedArticles.length === 0) return;
  setStatus(OPERATION_HEADINGS.check, "Looking up verified bibliographic metadata, then validating public PDF links. Records stay usable if a provider fails.", "running");
  setRunning("check");
  try {
    await invoke("check_metadata_pdfs");
  } catch (err) {
    finishOperation();
    if (String(err).includes("NOTHING_TO_CHECK")) {
      setStatus("Nothing to check", "Run a search first, then check metadata and public PDFs.", "warning");
    } else {
      taskError("check", String(err));
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
    year = parseInt(yearText, 10);
    if (!Number.isInteger(year) || year < 1500 || year > 2100) {
      setStatus("Check ranking year", "Ranking year must be a whole number, or empty when the file supplies it.", "error");
      return;
    }
  }
  setStatus(OPERATION_HEADINGS.import, "Validating the file in the background. A rejected file leaves the loaded records untouched.", "running");
  setRunning("import");
  try {
    await invoke("import_ranking", { path, defaultYear: year });
  } catch (err) {
    finishOperation();
    if (String(err).startsWith("No file found")) {
      setStatus("Check ranking file", String(err), "error");
    } else {
      taskError("import", String(err));
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
    taskError("download", String(err));
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
  const path = folder ? folder + "\\" + filename : filename;
  $("citationsDialog").close();
  setStatus(OPERATION_HEADINGS.citations, "Resolving citations against Crossref, then writing the file. Unresolved records keep their known fields and a review note.", "running");
  setRunning("citations");
  try {
    await invoke("export_citations", { articles: records, format: fmt, path });
  } catch (err) {
    finishOperation();
    taskError("citations", String(err));
  }
}

// ---- spreadsheet exports (synchronous on the UI thread) ----
async function exportSpreadsheet(kind) {
  const visible = visibleArticles().map(([, a]) => a);
  if (visible.length === 0) {
    setStatus("Ready", "No results to export.", "ready");
    return;
  }
  const folder = currentRequest?.outputFolder || $("folder").value.trim();
  const filename = kind === "excel" ? DEFAULT_EXPORT_NAMES.excel : DEFAULT_EXPORT_NAMES.csv;
  const path = folder ? folder + "\\" + filename : filename;
  const cmdName = kind === "excel" ? "export_excel" : "export_csv";
  try {
    const message = await invoke(cmdName, { articles: visible, path });
    const total = storedArticles.length;
    const detail = visible.length === total
      ? `Exported ${visible.length} result(s) to ${path}.`
      : `Exported ${visible.length} filtered result(s) of ${total} stored records to ${path}.`;
    setStatus("Ready", detail, "ready");
  } catch (err) {
    const msg = String(err).replace("Export failed: ", "");
    setStatus("Ready", `Export failed: ${msg}`, "error");
    alert(`Export failed\n\n${msg}`);
  }
}

// ---- table rendering (iid = article-{storedIndex}) ----
function filteredArticles() {
  return storedArticles
    .map((a, i) => [i, a])
    .filter(([, a]) => {
      if (pdfFilter === "All") return true;
      const want = pdfFilter === "Public PDF available" ? "available"
        : pdfFilter === "No public PDF found" ? "unavailable" : "unknown";
      return a.pdfStatus === want || a.pdf_status === want;
    });
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
    if (previousSelection.has(index)) tr.classList.add("selected");
    const title = a.title ?? "";
    const authors = a.authors ?? "";
    const score = a.relevanceScore ?? a.relevance_score;
    const scoreText = score == null ? "" : Number(score).toFixed(1);
    const rank = [a.sjrQuartile ?? a.sjr_quartile, a.sintaRank ?? a.sinta_rank]
      .filter(Boolean).join(" / ");
    const status = a.pdfStatus ?? a.pdf_status;
    const localPath = a.pdfLocalPath ?? a.pdf_local_path;
    const pdf = status === "available" ? (localPath ? "Downloaded" : "Public PDF")
      : status === "unavailable" ? "No public PDF" : "Unknown";
    const link = a.link ?? "";
    const linkText = link ? "Open link" : "No link";
    for (const text of [title, authors, scoreText, rank, pdf, linkText]) {
      const td = document.createElement("td");
      td.textContent = text;
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
      $("emptyDetail").textContent = "Change the PDF filter to see the stored records. Filtering never removes stored results.";
    } else {
      $("emptyTitle").textContent = "No results yet";
      $("emptyDetail").textContent = "Run a search to review Google Scholar results here.";
    }
  }
  refreshSummary();
  setIdleState();
}

function refreshSummary() {
  const rows = filteredArticles();
  const total = storedArticles.length;
  if (total === 0 || (pdfFilter === "All" && rows.length === total)) {
    $("summary").textContent = summaryBase;
  } else {
    $("summary").textContent = `${summaryBase} | filter '${pdfFilter}': ${rows.length} of ${total} shown`;
  }
}

function onTableClick(e) {
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
  setIdleState();
}

function onTableDblClick(e) {
  const tr = e.target.closest("tr[data-iid]");
  if (!tr) return;
  const index = parseInt(tr.dataset.iid.replace("article-", ""), 10);
  const a = storedArticles[index];
  if (a && (a.link || "")) window.open(a.link, "_blank");
}

// ---- details dialog ----
function openDetail() {
  if (selection.size === 0) {
    setStatus("No paper selected", "Select a row to see its journal, ranking and public-PDF details.", "warning");
    return;
  }
  const index = [...selection][0];
  const a = storedArticles[index];
  if (!a) return;
  $("detailText").textContent = articleDetailText(a);
  $("detailDialog").showModal();
}

function articleDetailText(a) {
  const none = "unknown";
  const lines = [a.title || "(untitled)"];
  lines.push(`Authors: ${a.authors || none}`);
  lines.push(`Link: ${a.link || none}`);
  lines.push(`DOI: ${a.doi || none}`);
  lines.push(`Journal: ${a.journal || none}`);
  lines.push(`ISSN: ${(a.issns ?? []).length ? a.issns.join(", ") : "none"}`);
  lines.push(`Publication year: ${a.publicationYear ?? a.publication_year ?? none}`);
  lines.push("");
  lines.push(`SJR quartile: ${a.sjrQuartile ?? a.sjr_quartile || none}`);
  if (a.sjrYear ?? a.sjr_year) lines.push(`SJR ranking year: ${a.sjrYear ?? a.sjr_year}`);
  lines.push(`SJR category: ${a.sjrCategory ?? a.sjr_category || none}`);
  lines.push(`SJR source: ${a.sjrSourceUrl ?? a.sjr_source_url || "no imported record"}`);
  lines.push(`SINTA rank: ${a.sintaRank ?? a.sinta_rank || none}`);
  if (a.sintaYear ?? a.sinta_year) lines.push(`SINTA ranking year: ${a.sintaYear ?? a.sinta_year}`);
  lines.push(`SINTA source: ${a.sintaSourceUrl ?? a.sinta_source_url || "no imported record"}`);
  lines.push("");
  const status = a.pdfStatus ?? a.pdf_status;
  const localPath = a.pdfLocalPath ?? a.pdf_local_path;
  lines.push(`Public PDF: ${status === "available" ? (localPath ? "Downloaded" : "Public PDF") : status === "unavailable" ? "No public PDF" : "Unknown"}`);
  lines.push(`PDF detail: ${a.pdfDetail ?? a.pdf_detail || "not checked"}`);
  lines.push(`PDF source: ${a.pdfSourceUrl ?? a.pdf_source_url || "none"}`);
  lines.push(`PDF local path: ${localPath || "not downloaded"}`);
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
      setStatus("Search failed", "The search did not return a result.", "error");
      return;
    }
    storedArticles = r.articles ?? [];
    currentRequest = r.request ?? currentRequest;
    const requested = r.requestedPages ?? r.requested_pages ?? 0;
    const successful = r.successfulPages ?? r.successful_pages ?? 0;
    const duplicates = r.duplicatesRemoved ?? r.duplicates_removed ?? 0;
    const invalid = r.invalidArticlesRemoved ?? r.invalid_articles_removed ?? 0;
    summaryBase = `${storedArticles.length} result(s) found | ${successful}/${requested} pages completed` +
      (duplicates ? ` | ${duplicates} duplicate(s) removed` : "") +
      (invalid ? ` | ${invalid} invalid record(s) removed` : "");
    const { from, to } = currentYearBounds();
    $("appliedYear").textContent = yearRangeLabel(from, to);
    setProgress(successful, requested);
    const message = r.message || STATUS_DEFAULTS[status] || "";
    setStatus(STATUS_HEADINGS[status] || "Ready", message, STATUS_TONES[status] || "ready");
    renderResults();
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
      if (p.cancelled) setStatus("Check cancelled", "The check was stopped; finished records keep their results.", "warning");
      else if (p.enrichError) setStatus("Check finished with errors", p.enrichError, "warning");
      else setStatus("Check complete", `${checked} of ${storedArticles.length} records now have a checked public-PDF status.`, "success");
      renderResults();
    } else if (op === "import") {
      rankingCount = p.count ?? 0;
      $("rankingNotice").textContent = rankingCount > 0 ? `${rankingCount} ranking records` : NO_RANKING_NOTICE;
      const year = p.year;
      setStatus("Rankings imported",
        `${p.count} ranking records${p.total ? ` (${p.total} total)` : ""}.` +
        (year ? ` Ranking year ${year} was used for records without one.` : " Records without a year stay unknown."),
        "success");
      renderResults();
    } else if (op === "download") {
      const indices = p.indices ?? [];
      const articles = p.articles ?? [];
      indices.forEach((storedIndex, i) => {
        if (storedArticles[storedIndex] && articles[i]) storedArticles[storedIndex] = articles[i];
      });
      const saved = articles.filter((a) => (a.pdfStatus ?? a.pdf_status) === "available").length;
      setStatus("Download complete", `${saved} PDF(s) saved to ${p.folder}.`, "success");
      renderResults();
    } else if (op === "citations") {
      if (p.cancelled) {
        setStatus("Export cancelled", "No citation file was written.", "warning");
      } else {
        setStatus("Export complete", `${p.verified} of ${p.count} record(s) verified; file written to ${p.path}.`, "success");
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
  // One of the two messagebox paths: alert + error status.
  const heading = OPERATION_HEADINGS[operation] || "Task failed";
  setStatus(heading, message, "error");
  alert(`${heading}\n\n${message}`);
}

function finishOperation() {
  // running predicate = worker liveness in Python; here: cleared on result.
  running = false;
  currentOperation = null;
  renderResults(); // re-enables controls per state
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
