"use strict";
// Kohaku Studio フロントエンド。外部ライブラリ非依存。
// UIはAPIにCommandを投げるだけで、データ処理はすべてRust側で行う。

const $ = (id) => document.getElementById(id);

// 細線のインラインSVGアイコン集(絵文字を使わずUIを軽く保つ。
// currentColor で描くので、CSS の色指定にそのまま追随する)。
const svg = (p) => `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${p}</svg>`;
const ICON = {
  moon: svg('<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z"/>'),
  sun: svg('<circle cx="12" cy="12" r="4.2"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M19 5l-1.5 1.5M6.5 17.5 5 19"/>'),
  left: svg('<path d="M15 6l-6 6 6 6"/>'),
  right: svg('<path d="M9 6l6 6-6 6"/>'),
  width: svg('<path d="M3 12h18M7 8l-4 4 4 4M17 8l4 4-4 4"/>'),
  height: svg('<path d="M12 3v18M8 7l4-4 4 4M8 17l4 4 4-4"/>'),
  folder: svg('<path d="M3 7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>'),
  file: svg('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z"/><path d="M14 3v5h5"/>'),
};

let datasets = [];
let charts = [];
let currentDataset = null;
let lastSqlResult = null;
let editingChartId = null;

// ---------- テーマ(ダーク / ライト) ----------
// 配色は style.css のCSS変数に集約し、切り替えは data-theme 属性の付け替えだけ。
// Canvasは色を自前で持つため、切替後に登録済みの再描画関数を呼び直す。

/** 描画済みCanvasの再描画関数。テーマ切替時にまとめて呼ぶ */
const REDRAW = new Map();

/** 各描画関数の先頭で呼び、同じ引数で描き直せるようにする */
function registerRedraw(canvas, fn) {
  REDRAW.set(canvas, fn);
}

function applyTheme(name) {
  const theme = name === "light" ? "light" : "dark";
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("kohaku.theme", theme);
  const btn = $("btn-theme");
  if (btn) {
    // 押すと切り替わる先を示す(ダーク表示中は「ライトへ」の太陽アイコン)
    btn.innerHTML = theme === "dark" ? ICON.sun : ICON.moon;
    btn.title = theme === "dark" ? "ライトテーマに切り替え" : "ダークテーマに切り替え";
  }
  refreshThemeColors();
  // DOMから外れたCanvas(再生成されたダッシュボード等)は登録を捨てる
  for (const [canvas, fn] of [...REDRAW]) {
    if (!canvas.isConnected) {
      REDRAW.delete(canvas);
      continue;
    }
    try {
      fn();
    } catch (e) {
      /* 再描画に失敗しても操作は続行できる */
    }
  }
}

function initTheme() {
  // ?theme=light / dark で明示指定できる(ブックマークやスクリーンショット用)。
  // 指定がなければ前回の選択、それも無ければOSの設定に従う(既定はダーク)。
  const forced = new URLSearchParams(location.search).get("theme");
  const saved = localStorage.getItem("kohaku.theme");
  const prefersLight = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches;
  applyTheme(forced || saved || (prefersLight ? "light" : "dark"));
}

// ---------- 共通 ----------

async function api(path, body) {
  const r = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
  const j = await r.json();
  if (!j.ok) throw new Error(j.error || "APIエラー");
  return j.data;
}

function setStatus(msg, isError) {
  const el = $("status");
  el.textContent = msg;
  el.className = isError ? "error" : "";
}

function esc(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function qi(name) {
  // SQLite識別子クオート
  return '"' + String(name).replace(/"/g, '""') + '"';
}

function fmtCell(v) {
  if (v === null || v === undefined) return { text: "null", cls: "null" };
  if (typeof v === "number") {
    const text = Number.isInteger(v) ? v.toLocaleString() : String(Math.round(v * 10000) / 10000);
    return { text, cls: "num" };
  }
  return { text: String(v), cls: "" };
}

function renderTable(container, columns, rows, maxRows) {
  maxRows = maxRows || 500;
  const shown = rows.slice(0, maxRows);
  let html = '<table class="grid"><thead><tr>';
  for (const c of columns) html += `<th>${esc(typeof c === "string" ? c : c.name + " (" + c.type + ")")}</th>`;
  html += "</tr></thead><tbody>";
  for (const row of shown) {
    html += "<tr>";
    for (const v of row) {
      const f = fmtCell(v);
      html += `<td class="${f.cls}">${esc(f.text)}</td>`;
    }
    html += "</tr>";
  }
  html += "</tbody></table>";
  if (rows.length > maxRows) html += `<div class="hint">${rows.length.toLocaleString()}行中 先頭${maxRows}行を表示</div>`;
  container.innerHTML = html;
}

function switchTab(name) {
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.tab === name));
  document.querySelectorAll(".pane").forEach((p) => p.classList.toggle("active", p.id === "tab-" + name));
  if (name === "dashboard") renderDashboard();
  if (name === "analytics") {
    renderAnDatasetSelect();
    if (!anColumns.length) anLoadColumns();
  }
}

// ---------- データセット ----------

async function refreshState() {
  const st = await api("/api/state");
  datasets = st.datasets || [];
  charts = st.charts || [];
  $("project-name").textContent = st.project_name || "無題プロジェクト";
  renderSidebar();
  renderChartList();
  renderDatasetSelect();
  renderAnDatasetSelect();
  renderLotColSelect();
  renderHistory(st.queries || []);
  // データセット一覧が変わった可能性があるため、チャートの列セレクトも追随させる。
  // これを怠ると「インポート直後にソースを切り替えるまでX列/Y列が選べない」状態になる
  await loadChartColumns();
}

/** 表示用: 接続URLのパスワード部分を伏せる */
function maskSecret(p) {
  return String(p).replace(/^(\w+:\/\/[^:/@]+):[^@]+@/, "$1:••••@");
}

function renderSidebar() {
  const ul = $("dataset-list");
  ul.innerHTML = "";
  for (const d of datasets) {
    const li = document.createElement("li");
    li.classList.toggle("active", d.name === currentDataset);
    li.innerHTML = `<span class="ds-name" title="${esc(maskSecret(d.path))}">${esc(d.name)}</span>
      <span class="ds-rows">${(d.row_count || 0).toLocaleString()}行</span>
      <button class="ds-del" title="削除">×</button>`;
    li.querySelector(".ds-name").onclick = () => showDataset(d.name);
    li.querySelector(".ds-del").onclick = async (e) => {
      e.stopPropagation();
      if (!confirm(`データセット「${d.name}」を削除しますか?`)) return;
      try {
        await api("/api/dataset/delete", { name: d.name });
        if (currentDataset === d.name) currentDataset = null;
        await refreshState();
        setStatus(`削除しました: ${d.name}`);
      } catch (err) {
        setStatus(err.message, true);
      }
    };
    ul.appendChild(li);
  }
}

async function showDataset(name) {
  currentDataset = name;
  renderSidebar();
  switchTab("data");
  const d = datasets.find((x) => x.name === name);
  try {
    const r = await api("/api/query", { sql: `SELECT * FROM ${qi(name)} LIMIT 200`, limit: 200 });
    const schemaText = (d && d.schema)
      ? d.schema.columns.map((c) => `${c.name}:${c.data_type === "Int64" ? "整数" : c.data_type === "Float64" ? "実数" : c.data_type === "Utf8" ? "文字列" : c.data_type === "Boolean" ? "真偽" : c.data_type}`).join("、 ")
      : "";
    $("data-info").innerHTML = `<b>${esc(name)}</b> — ${(d ? d.row_count : 0).toLocaleString()}行<br><span class="hint">${esc(schemaText)}</span>`;
    renderTable($("data-table"), r.columns, r.rows, 200);
  } catch (err) {
    setStatus(err.message, true);
  }
}

// ---------- インポートウィザード ----------

const imp = { dir: "", path: "", connector: "", objects: [] };

function openImport() {
  $("import-modal").classList.remove("hidden");
  $("imp-config").classList.add("hidden");
  $("imp-msg").textContent = "";
  $("imp-preview").innerHTML = "";
  $("imp-db-url").value = localStorage.getItem("kohaku.lastDbUrl") || "";
  impMode(localStorage.getItem("kohaku.impMode") || "file");
  browse(localStorage.getItem("kohaku.lastDir") || "");
}

/** インポート元の切替: ファイル / データベース */
function impMode(mode) {
  localStorage.setItem("kohaku.impMode", mode);
  $("imp-tab-file").classList.toggle("active", mode === "file");
  $("imp-tab-db").classList.toggle("active", mode === "db");
  $("imp-file-panel").classList.toggle("hidden", mode !== "file");
  $("imp-db-panel").classList.toggle("hidden", mode !== "db");
  $("imp-config").classList.add("hidden");
  $("imp-db-msg").textContent = "";
  $("imp-preview").innerHTML = "";
  $("imp-msg").textContent = "";
}

/** DB接続URLからテーブル一覧を取得してインポート設定を表示する */
async function connectDb() {
  const url = $("imp-db-url").value.trim();
  if (!url) {
    $("imp-db-msg").textContent = "接続URLを入力してください";
    return;
  }
  $("imp-db-msg").textContent = "接続中...";
  try {
    const r = await api("/api/objects", { path: url });
    if (!r.objects.length) {
      $("imp-db-msg").textContent = "テーブルが見つかりません(スキーマ・権限を確認してください)";
      return;
    }
    localStorage.setItem("kohaku.lastDbUrl", url);
    imp.path = url;
    imp.connector = r.connector;
    imp.objects = r.objects;
    const sel = $("imp-object");
    sel.innerHTML = "";
    for (const o of r.objects) {
      const op = document.createElement("option");
      op.value = o;
      op.textContent = o;
      sel.appendChild(op);
    }
    $("imp-delim-row").classList.add("hidden");
    $("imp-name").value = sanitizeName(r.objects[0]);
    $("imp-db-msg").textContent = `${r.connector} に接続しました(${r.objects.length}テーブル)`;
    $("imp-config").classList.remove("hidden");
    await refreshImportPreview();
  } catch (err) {
    $("imp-db-msg").textContent = err.message;
  }
}

async function browse(path) {
  try {
    const r = await api("/api/browse", { path });
    imp.dir = r.path;
    localStorage.setItem("kohaku.lastDir", r.path);
    $("imp-path").value = r.path;
    const ul = $("imp-entries");
    ul.innerHTML = "";
    if (r.parent) {
      const li = document.createElement("li");
      li.className = "imp-dir";
      li.innerHTML = `<span class="imp-ico">${ICON.folder}</span><span>..</span>`;
      li.onclick = () => browse(r.parent);
      ul.appendChild(li);
    }
    for (const d of r.dirs) {
      const li = document.createElement("li");
      li.className = "imp-dir";
      li.innerHTML = `<span class="imp-ico">${ICON.folder}</span><span>${esc(d)}</span>`;
      li.onclick = () => browse(r.path + "\\" + d);
      ul.appendChild(li);
    }
    for (const f of r.files) {
      const li = document.createElement("li");
      const kb = Math.max(1, Math.round(f.size / 1024));
      li.innerHTML = `<span class="imp-name"><span class="imp-ico">${ICON.file}</span>${esc(f.name)}</span><span class="fsize">${kb.toLocaleString()} KB</span>`;
      li.onclick = () => {
        ul.querySelectorAll("li").forEach((x) => x.classList.remove("sel"));
        li.classList.add("sel");
        selectFile(f.name);
      };
      ul.appendChild(li);
    }
  } catch (err) {
    setStatus(err.message, true);
  }
}

async function selectFile(name) {
  imp.path = imp.dir.replace(/[\\/]$/, "") + "\\" + name;
  try {
    const r = await api("/api/objects", { path: imp.path });
    imp.connector = r.connector;
    imp.objects = r.objects;
    const sel = $("imp-object");
    sel.innerHTML = "";
    for (const o of r.objects) {
      const op = document.createElement("option");
      op.value = o;
      op.textContent = o;
      sel.appendChild(op);
    }
    $("imp-delim-row").classList.toggle("hidden", r.connector !== "csv");
    const stem = name.replace(/\.[^.]+$/, "");
    $("imp-name").value = sanitizeName(r.connector === "excel" && r.objects.length > 1 ? stem + "_" + r.objects[0] : stem);
    $("imp-config").classList.remove("hidden");
    await refreshImportPreview();
  } catch (err) {
    setStatus(err.message, true);
  }
}

function importOptions() {
  const delim = $("imp-delim").value;
  return {
    header_row: parseInt($("imp-header").value, 10) || 0,
    skip_rows: 0,
    delimiter: delim === "" ? null : delim,
  };
}

function sanitizeName(s) {
  return s.replace(/[^\w-￿]/g, "_");
}

async function refreshImportPreview() {
  if (!imp.path) return;
  $("imp-msg").textContent = "プレビュー読み込み中...";
  try {
    const r = await api("/api/preview", {
      path: imp.path,
      object: $("imp-object").value,
      options: Object.assign(importOptions(), { max_rows: 50 }),
    });
    renderTable($("imp-preview"), r.columns, r.rows, 50);
    $("imp-msg").textContent = "";
  } catch (err) {
    $("imp-msg").textContent = err.message;
  }
}

async function doImport() {
  if (!imp.path) return;
  $("imp-msg").textContent = "取り込み中...";
  try {
    const r = await api("/api/import", {
      path: imp.path,
      object: $("imp-object").value,
      name: $("imp-name").value,
      options: importOptions(),
    });
    $("import-modal").classList.add("hidden");
    await refreshState();
    setStatus(`インポート完了: ${r.name} (${r.rows.toLocaleString()}行)`);
    showDataset(r.name);
  } catch (err) {
    $("imp-msg").textContent = err.message;
  }
}

// ---------- SQL ----------

// シンタックスハイライト。外部ライブラリを使わず、透明にした textarea の
// 背面に色付きの <pre> を重ねて描く(編集・IME・取り消しは textarea のまま)。

/** 色を付けるSQLキーワード(SQLiteの基本構文 + よく使う関数・型) */
const SQL_KEYWORDS = new Set(
  ("SELECT FROM WHERE GROUP BY ORDER HAVING LIMIT OFFSET JOIN INNER LEFT RIGHT FULL OUTER CROSS ON " +
   "AS AND OR NOT IN LIKE GLOB BETWEEN IS NULL EXISTS CASE WHEN THEN ELSE END DISTINCT ALL UNION " +
   "INTERSECT EXCEPT WITH RECURSIVE INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE VIEW INDEX " +
   "DROP ALTER ADD PRIMARY KEY FOREIGN REFERENCES UNIQUE CHECK DEFAULT AUTOINCREMENT CAST COLLATE " +
   "ASC DESC NULLS FIRST LAST OVER PARTITION WINDOW ROWS RANGE PRECEDING FOLLOWING CURRENT ROW " +
   "COUNT SUM AVG MIN MAX ABS ROUND COALESCE IFNULL NULLIF SUBSTR LENGTH UPPER LOWER TRIM REPLACE " +
   "DATE TIME DATETIME STRFTIME JULIANDAY INTEGER REAL TEXT BLOB NUMERIC BOOLEAN TRUE FALSE").split(" ")
);

/** SQLを色付きHTMLへ変換する。優先順は コメント → 文字列 → 引用識別子 →
 *  数値 → 語(キーワード判定)。コメントと文字列を先に食わせないと、
 *  その中のキーワードまで色付いてしまう。 */
function highlightSql(src) {
  // 1:ブロックコメント 2:行コメント 3:文字列 4:引用識別子 5:数値 6:語 7:その他
  const re = /(\/\*[\s\S]*?(?:\*\/|$))|(--[^\n]*)|('(?:''|[^'])*'?)|("(?:""|[^"])*"?)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z_0-9]*)|([\s\S])/g;
  let out = "";
  let m;
  while ((m = re.exec(src)) !== null) {
    const [t, block, line, str, ident, num, word] = m;
    if (block || line) out += `<span class="sql-c">${esc(t)}</span>`;
    else if (str) out += `<span class="sql-s">${esc(t)}</span>`;
    else if (ident) out += `<span class="sql-i">${esc(t)}</span>`;
    else if (num) out += `<span class="sql-n">${esc(t)}</span>`;
    else if (word) {
      out += SQL_KEYWORDS.has(word.toUpperCase()) ? `<span class="sql-k">${esc(t)}</span>` : esc(t);
    } else out += esc(t);
  }
  return out;
}

/** 背面のハイライトを本文とスクロール位置に追従させる */
function syncSqlHighlight() {
  const ta = $("sql-input");
  const hl = $("sql-hl");
  if (!ta || !hl) return;
  // 末尾の改行は <pre> だと潰れて高さがずれるので、1文字ぶん足しておく
  hl.innerHTML = highlightSql(ta.value) + "\n";
  hl.scrollTop = ta.scrollTop;
  hl.scrollLeft = ta.scrollLeft;
}

async function runSql() {
  const sql = $("sql-input").value.trim();
  if (!sql) return;
  $("sql-msg").textContent = "実行中...";
  $("sql-msg").className = "hint";
  const t0 = performance.now();
  try {
    const r = await api("/api/query", { sql, limit: 2000 });
    lastSqlResult = r;
    const ms = Math.round(performance.now() - t0);
    $("sql-msg").textContent = `${r.total_returned.toLocaleString()}行${r.truncated ? "(2000行で打ち切り)" : ""} — ${ms}ms`;
    renderTable($("sql-table"), r.columns, r.rows, 500);
    const st = await api("/api/state");
    renderHistory(st.queries || []);
  } catch (err) {
    $("sql-msg").textContent = err.message;
    $("sql-msg").className = "hint error";
  }
}

function renderHistory(queries) {
  const sel = $("sql-history");
  sel.innerHTML = '<option value="">履歴...</option>';
  for (const q of queries) {
    const op = document.createElement("option");
    op.value = q;
    op.textContent = q.length > 60 ? q.slice(0, 60) + "…" : q;
    sel.appendChild(op);
  }
}

function exportCsv() {
  if (!lastSqlResult) return;
  const { columns, rows } = lastSqlResult;
  const escape = (v) => {
    if (v === null || v === undefined) return "";
    const s = String(v);
    return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  };
  let csv = columns.map(escape).join(",") + "\n";
  for (const r of rows) csv += r.map(escape).join(",") + "\n";
  const blob = new Blob(["﻿" + csv], { type: "text/csv" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "query_result.csv";
  a.click();
  URL.revokeObjectURL(a.href);
}

// ---------- チャート ----------

function chartSpecFromForm() {
  return {
    id: editingChartId || Date.now(),
    name: $("ch-name").value.trim() || "チャート",
    chart_type: $("ch-type").value,
    source: $("ch-source-kind").value === "dataset"
      ? { kind: "dataset", dataset: $("ch-dataset").value }
      : { kind: "sql", sql: $("ch-sql").value.trim() },
    x: $("ch-x").value,
    y: $("ch-y").value,
    value: $("ch-value").value,
    series: $("ch-series").value,
    facet: $("ch-facet").value,
    facet2: $("ch-facet2").value,
    agg: $("ch-agg").value,
    bins: parseInt($("ch-bins").value, 10) || 20,
    // 軸の手動レンジ(空欄なら自動)
    y_min: $("ch-ymin").value.trim(),
    y_max: $("ch-ymax").value.trim(),
    x_min: $("ch-xmin").value.trim(),
    x_max: $("ch-xmax").value.trim(),
  };
}

function loadChartToForm(spec) {
  editingChartId = spec.id;
  $("ch-name").value = spec.name || "";
  $("ch-type").value = spec.chart_type || "bar";
  $("ch-source-kind").value = spec.source && spec.source.kind === "sql" ? "sql" : "dataset";
  if (spec.source && spec.source.kind === "sql") $("ch-sql").value = spec.source.sql || "";
  updateChartFormVisibility();
  renderDatasetSelect();
  if (spec.source && spec.source.kind === "dataset") $("ch-dataset").value = spec.source.dataset || "";
  loadChartColumns().then(() => {
    $("ch-x").value = spec.x || "";
    $("ch-y").value = spec.y || "";
    $("ch-value").value = spec.value || "";
    $("ch-series").value = spec.series || "";
    $("ch-facet").value = spec.facet || "";
    $("ch-facet2").value = spec.facet2 || "";
    syncFacet2Enabled();
    $("ch-agg").value = spec.agg || "none";
    $("ch-bins").value = spec.bins || 20;
    $("ch-ymin").value = spec.y_min ?? "";
    $("ch-ymax").value = spec.y_max ?? "";
    $("ch-xmin").value = spec.x_min ?? "";
    $("ch-xmax").value = spec.x_max ?? "";
    previewChart();
  });
  renderChartList();
}

/** 行(facet2)は列(facet)を選んだときだけ使える(2つ目の分割軸のため)。
 *  列が空なら行セレクタを無効化して、順序を明示する。 */
function syncFacet2Enabled() {
  $("ch-facet2").disabled = !$("ch-facet").value;
}

function updateChartFormVisibility() {
  const kind = $("ch-source-kind").value;
  const type = $("ch-type").value;
  syncFacet2Enabled();
  $("ch-dataset-row").classList.toggle("hidden", kind !== "dataset");
  $("ch-sql-row").classList.toggle("hidden", kind !== "sql");
  const reg = CHART_REGISTRY.get(type);
  if (reg) {
    // 登録チャート: form 定義に従って行の表示とラベルを切り替える
    const f = reg.form || {};
    $("ch-bins-row").classList.add("hidden");
    $("ch-x-row").classList.toggle("hidden", !f.x);
    $("ch-y-row").classList.toggle("hidden", !f.y);
    $("ch-value-row").classList.toggle("hidden", !f.value);
    $("ch-series-row").classList.toggle("hidden", !f.series);
    $("ch-agg-row").classList.toggle("hidden", !f.agg);
    $("ch-x-label").textContent = typeof f.x === "string" ? f.x : "X列";
    $("ch-y-label").textContent = typeof f.y === "string" ? f.y : "Y列";
    $("ch-value-label").textContent = typeof f.value === "string" ? f.value : "値の列";
    $("ch-yrange-row").classList.toggle("hidden", !f.yrange);
    $("ch-xrange-row").classList.toggle("hidden", !f.xrange);
    $("ch-facet-row").classList.toggle("hidden", !f.facet);
    $("ch-facet2-row").classList.toggle("hidden", !f.facet);
    return;
  }
  $("ch-x-label").textContent = "X列";
  $("ch-y-label").textContent = "Y列";
  $("ch-value-row").classList.add("hidden");
  $("ch-bins-row").classList.toggle("hidden", type !== "histogram");
  $("ch-y-row").classList.toggle("hidden", type === "histogram" || type === "table");
  $("ch-x-row").classList.toggle("hidden", type === "table");
  $("ch-series-row").classList.toggle("hidden", type === "table");
  $("ch-facet-row").classList.toggle("hidden", type === "table");
  $("ch-facet2-row").classList.toggle("hidden", type === "table");
  $("ch-agg-row").classList.toggle("hidden", type === "histogram" || type === "table" || type === "scatter");
  $("ch-yrange-row").classList.toggle("hidden", type === "table");
  // X軸の手動レンジは、X軸が数値になるグラフ(散布図・折れ線・ヒストグラム)でのみ意味を持つ
  $("ch-xrange-row").classList.toggle(
    "hidden",
    !["scatter", "line", "histogram"].includes(type),
  );
}

function renderDatasetSelect() {
  const sel = $("ch-dataset");
  const cur = sel.value;
  sel.innerHTML = "";
  for (const d of datasets) {
    const op = document.createElement("option");
    op.value = d.name;
    op.textContent = d.name;
    sel.appendChild(op);
  }
  if (cur && datasets.some((d) => d.name === cur)) sel.value = cur;
}

function stripSemi(sql) {
  return sql.trim().replace(/;+\s*$/, "");
}

function chartBaseSql(spec) {
  return spec.source.kind === "dataset"
    ? `SELECT * FROM ${qi(spec.source.dataset)}`
    : stripSemi(spec.source.sql);
}

async function loadChartColumns() {
  const kind = $("ch-source-kind").value;
  let cols = [];
  try {
    if (kind === "dataset") {
      const d = datasets.find((x) => x.name === $("ch-dataset").value);
      if (d && d.schema) cols = d.schema.columns.map((c) => c.name);
    } else {
      const sql = stripSemi($("ch-sql").value);
      if (sql) {
        const r = await api("/api/query", { sql: `SELECT * FROM (${sql}) LIMIT 1`, limit: 1 });
        cols = r.columns;
      }
    }
  } catch (e) {
    /* SQL未完成時は無視 */
  }
  for (const id of ["ch-x", "ch-y", "ch-value", "ch-series", "ch-facet", "ch-facet2"]) {
    const sel = $(id);
    const cur = sel.value;
    sel.innerHTML = "";
    if (id === "ch-series" || id === "ch-facet" || id === "ch-facet2") {
      // 系列・ファセットは任意指定(既定はなし)
      const none = document.createElement("option");
      none.value = "";
      none.textContent = "(なし)";
      sel.appendChild(none);
    }
    for (const c of cols) {
      const op = document.createElement("option");
      op.value = c;
      op.textContent = c;
      sel.appendChild(op);
    }
    if (cols.includes(cur)) sel.value = cur;
  }
  syncFacet2Enabled();
}

/** SQLリテラル化(数値は素通し、文字列は''エスケープ) */
function sqlLit(v) {
  if (typeof v === "number") return String(v);
  if (typeof v === "boolean") return v ? "1" : "0";
  return "'" + String(v).replace(/'/g, "''") + "'";
}

/** チャートの元クエリにグローバルフィルタ(WHERE col IN ...)を重ねる */
function filteredBase(spec, filters) {
  let base = chartBaseSql(spec);
  const conds = (filters || [])
    .filter((f) => f.values.length)
    .map((f) => `${qi(f.col)} IN (${f.values.map(sqlLit).join(", ")})`);
  if (conds.length) base = `SELECT * FROM (${base}) WHERE ${conds.join(" AND ")}`;
  return base;
}

function buildChartQuery(spec, filters) {
  const base = filteredBase(spec, filters);
  const reg = CHART_REGISTRY.get(spec.chart_type);
  if (reg) return reg.buildQuery(spec, base);
  const x = qi(spec.x), y = qi(spec.y);
  // 系列列(任意)。指定時は s 列としてSELECTに含める
  const s = spec.series ? `, ${qi(spec.series)} AS s` : "";
  // ファセット列(任意)。f=列(横に並べる) / f2=行(縦に並べる)
  const f = spec.facet ? `, ${qi(spec.facet)} AS f` : "";
  const f2 = spec.facet && spec.facet2 ? `, ${qi(spec.facet2)} AS f2` : "";
  switch (spec.chart_type) {
    case "table":
      return `SELECT * FROM (${base}) LIMIT 500`;
    case "histogram":
      return `SELECT ${x} AS x${s}${f}${f2} FROM (${base}) WHERE ${x} IS NOT NULL LIMIT 100000`;
    case "scatter":
      return `SELECT ${x} AS x, ${y} AS y${s}${f}${f2} FROM (${base}) WHERE ${x} IS NOT NULL AND ${y} IS NOT NULL LIMIT 20000`;
    default: {
      if (spec.agg === "none") {
        return `SELECT ${x} AS x, ${y} AS y${s}${f}${f2} FROM (${base}) LIMIT 20000`;
      }
      const agg = spec.agg === "count" ? "COUNT(*)" : `${spec.agg.toUpperCase()}(${y})`;
      const grp = [
        x,
        spec.series ? qi(spec.series) : null,
        spec.facet ? qi(spec.facet) : null,
        spec.facet && spec.facet2 ? qi(spec.facet2) : null,
      ]
        .filter(Boolean)
        .join(", ");
      // ファセット時はグループ数が増えるため上限を広げる
      const lim = spec.facet ? 12000 : 4000;
      return `SELECT ${x} AS x, ${agg} AS y${s}${f}${f2} FROM (${base}) GROUP BY ${grp} ORDER BY ${x} LIMIT ${lim}`;
    }
  }
}

/** チャートのデータ取得。通常はSQL(buildChartQuery)を実行するが、登録チャートは
 *  fetch フック(分析API等を呼ぶ非同期処理)を持てる(SPC管理図が利用) */
async function chartData(spec, filters) {
  const reg = CHART_REGISTRY.get(spec.chart_type);
  if (reg && reg.fetch) return reg.fetch(spec, filteredBase(spec, filters));
  return api("/api/query", { sql: buildChartQuery(spec, filters), limit: 100000 });
}

async function previewChart() {
  const spec = chartSpecFromForm();
  $("chart-title").textContent = spec.name;
  $("chart-msg").textContent = "";
  try {
    const r = await chartData(spec);
    drawChartInto($("chart-canvas"), $("chart-table"), spec, r);
  } catch (err) {
    $("chart-msg").textContent = err.message;
  }
}

function drawChartInto(canvas, tableDiv, spec, result) {
  registerRedraw(canvas, () => drawChartInto(canvas, tableDiv, spec, result));
  if (spec.chart_type === "table") {
    canvas.classList.add("hidden");
    tableDiv.classList.remove("hidden");
    renderTable(tableDiv, result.columns, result.rows, 500);
  } else {
    canvas.classList.remove("hidden");
    tableDiv.classList.add("hidden");
    renderChart(canvas, spec, result);
  }
}

async function saveChart() {
  const spec = chartSpecFromForm();
  const idx = charts.findIndex((c) => c.id === spec.id);
  if (idx >= 0) {
    spec.layout = charts[idx].layout; // レイアウト設定は編集で失わない
    charts[idx] = spec;
  } else {
    charts.push(spec);
  }
  dashCache.delete(spec.id); // 定義が変わったのでキャッシュ破棄
  dashSourceCols.delete("sql:" + spec.id);
  editingChartId = spec.id;
  try {
    await api("/api/charts/set", { charts });
    renderChartList();
    setStatus(`チャートを保存しました: ${spec.name}`);
  } catch (err) {
    setStatus(err.message, true);
  }
}

function renderChartList() {
  const ul = $("chart-list");
  ul.innerHTML = "";
  for (const c of charts) {
    const li = document.createElement("li");
    li.classList.toggle("active", c.id === editingChartId);
    li.innerHTML = `<span>${esc(c.name)}</span><button class="ch-del" title="削除">×</button>`;
    li.querySelector("span").onclick = () => loadChartToForm(c);
    li.querySelector(".ch-del").onclick = async (e) => {
      e.stopPropagation();
      charts = charts.filter((x) => x.id !== c.id);
      dashCache.delete(c.id);
      dashSourceCols.delete("sql:" + c.id);
      if (editingChartId === c.id) editingChartId = null;
      await api("/api/charts/set", { charts });
      renderChartList();
    };
    ul.appendChild(li);
  }
}

// ---------- Canvasチャートレンダラ ----------

// チャートの色は style.css のテーマ変数から読む(テーマ切替に追随させるため)。
// 参照側が持つ配列・オブジェクトを壊さないよう、再代入せず中身を書き換える。
const CHART_COLORS = { accent: "", accent2: "", text: "", grid: "", axis: "", brand: "", danger: "", ok: "", warn: "", muted: "" };
/** 系列の色パレット(最大8系列) */
const SERIES_COLORS = [];

/** テーマ変更時にCSS変数から色を読み直す */
function refreshThemeColors() {
  CHART_COLORS.accent = cssVar("--chart-primary", "#4f8ef7"); // データの既定色(UIのブランド色とは別)
  CHART_COLORS.accent2 = cssVar("--accent2", "#4fc4a0");
  CHART_COLORS.text = cssVar("--chart-text", "#a8b0be");
  CHART_COLORS.grid = cssVar("--chart-grid", "#313743");
  CHART_COLORS.axis = cssVar("--chart-axis", "#4a5361");
  CHART_COLORS.brand = cssVar("--accent", "#e8a33d");
  CHART_COLORS.danger = cssVar("--danger", "#e5707a");
  CHART_COLORS.ok = cssVar("--ok", "#4fc4a0");
  CHART_COLORS.warn = cssVar("--warn", "#e0a15c");
  CHART_COLORS.muted = cssVar("--muted", "#8a93a3");
  SERIES_COLORS.length = 0;
  for (let i = 1; i <= 8; i++) SERIES_COLORS.push(cssVar(`--series-${i}`, "#4f8ef7"));
}

/** ファセット1パネルに最低限確保する高さ(軸ラベルまで読める大きさ) */
const FACET_MIN_PANEL_H = 190;
/** ファセットで伸ばすCanvasの上限(際限なく伸びるのを防ぐ) */
const FACET_MAX_CANVAS_H = 1600;

/** ファセットの段数を数える(表示上限で打ち切ったあとの実際の段数)。
 *  0 = ファセットなし。 */
function facetRowCount(spec, result) {
  if (!spec.facet) return 0;
  const reg = CHART_REGISTRY.get(spec.chart_type);
  const twoD = !!spec.facet2;
  const uniq = (vals) => new Set(vals.map((v) => (v === null ? "(null)" : String(v)))).size;
  // 1次元は行数=ceil(パネル数/列数)、2次元は行変数の値の数がそのまま段数
  const gridRows = (n) => (n ? Math.ceil(n / facetCols(n)) : 0);
  if (Array.isArray(result.groups)) {
    // fetch チャート(SPC): サーバーが分割済み
    if (twoD && result.group2) {
      return Math.min(uniq(result.groups.map((g) => g.value2)), FACET2_DIM_MAX);
    }
    return gridRows(result.groups.length);
  }
  const f2i = result.columns ? result.columns.indexOf("f2") : -1;
  const fi = result.columns ? result.columns.indexOf("f") : -1;
  if (twoD && f2i >= 0) {
    return Math.min(uniq(result.rows.map((r) => r[f2i])), FACET2_DIM_MAX);
  }
  if (fi >= 0) {
    const max = (reg && reg.form && reg.form.facetMax) || 12;
    return gridRows(Math.min(uniq(result.rows.map((r) => r[fi])), max));
  }
  return 0;
}

/** ファセットの段数に応じてCanvasを縦に伸ばす。
 *  段数で高さを等分するだけだと、3段以上でパネルが潰れて軸ラベルすら
 *  読めなくなるため、1パネルの最低高さを確保できるところまで伸ばす
 *  (CSSの既定高さより低くはしない。ファセットなしなら既定へ戻す)。 */
function fitCanvasToFacets(canvas, spec, result) {
  canvas.style.height = ""; // まずCSSの既定に戻してから基準の高さを測る
  const rows = facetRowCount(spec, result);
  if (rows < 2) return;
  const base = canvas.clientHeight || 400;
  // 2次元は上にキャプション+列見出しの帯が乗るぶんを足す
  const strip = spec.facet2 ? 50 : 12;
  const need = Math.min(FACET_MAX_CANVAS_H, rows * FACET_MIN_PANEL_H + strip);
  if (need > base) canvas.style.height = `${need}px`;
}

function setupCanvas(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth || 600;
  const h = canvas.clientHeight || 400;
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.font = "11px 'Yu Gothic UI', sans-serif";
  return { ctx, w, h };
}

/** 軸の目盛り。返した目盛りの最小〜最大が描画レンジになるため、
 *  データ範囲[min, max]を必ず覆うように両端をステップの倍数へ広げる。
 *  (片側だけ広げるとプロットが枠外に飛び出し、軸ラベルとも重なる) */
function niceTicks(min, max, count) {
  if (!isFinite(min) || !isFinite(max)) return [0, 1];
  if (min > max) [min, max] = [max, min];
  if (min === max) { min -= 1; max += 1; }
  const step0 = (max - min) / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(step0)));
  let step = mag * 10;
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (mag * m >= step0) { step = mag * m; break; }
  }
  const eps = step * 1e-9; // 浮動小数の誤差でデータ端が外れないように緩める
  const lo = Math.floor((min + eps) / step) * step;
  const hi = Math.ceil((max - eps) / step) * step;
  const n = Math.max(1, Math.round((hi - lo) / step));
  const ticks = [];
  // 加算の誤差蓄積を避けるため、インデックスから直接計算する
  for (let i = 0; i <= n; i++) ticks.push(Math.round((lo + i * step) * 1e9) / 1e9);
  return ticks;
}

/** 指定レンジ[min, max]の内側だけに目盛りを置く(範囲は広げない)。
 *  手動レンジ指定用。自動レンジは niceTicks(範囲を広げてデータを覆う)を使う。 */
function ticksWithin(min, max, count) {
  if (!isFinite(min) || !isFinite(max) || !(max > min)) return [min, max];
  const step0 = (max - min) / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(step0)));
  let step = mag * 10;
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (mag * m >= step0) { step = mag * m; break; }
  }
  const eps = step * 1e-9;
  const ticks = [];
  const first = Math.ceil((min - eps) / step);
  for (let i = first; i * step <= max + eps; i++) ticks.push(Math.round(i * step * 1e9) / 1e9);
  // 目盛りが1本も入らない極端なレンジでは両端を使う
  return ticks.length >= 2 ? ticks : [min, max];
}

/** 数値として有効なら返す。空欄・不正値は null(=自動) */
function numOrNull(v) {
  if (v === null || v === undefined || String(v).trim() === "") return null;
  const n = Number(v);
  return isFinite(n) ? n : null;
}

/** 自動計算した目盛りに、spec の手動レンジを重ねる。既定はY軸(y_min / y_max)、
 *  axis="x" でX軸(x_min / x_max)を見る。片側だけの指定も可。
 *  不正(最小≧最大)なら自動のままにする。戻り値: {ticks, min, max} */
function applyManualRange(spec, autoTicks, count, axis) {
  const auto = { ticks: autoTicks, min: autoTicks[0], max: autoTicks[autoTicks.length - 1] };
  const key = axis === "x" ? "x" : "y";
  const lo = numOrNull(spec && spec[`${key}_min`]);
  const hi = numOrNull(spec && spec[`${key}_max`]);
  if (lo === null && hi === null) return auto;
  const min = lo !== null ? lo : auto.min;
  const max = hi !== null ? hi : auto.max;
  if (!(max > min)) return auto; // 逆転・同値は無視して自動に戻す
  return { ticks: ticksWithin(min, max, count || 5), min, max };
}

function fmtTick(v) {
  if (Math.abs(v) >= 1e6) return (v / 1e6) + "M";
  if (Math.abs(v) >= 1e4) return (v / 1e3) + "k";
  return String(Math.round(v * 1000) / 1000);
}

/** Y軸ラベル: 何をプロットしているかを常に明示する */
function chartYLabel(spec) {
  if (spec.chart_type === "histogram") return "度数";
  // 散布図は集計しない(フォームに残った集計値を無視する)
  if (spec.chart_type === "scatter") return spec.y || "";
  if (spec.agg === "count") return "件数";
  if (spec.agg && spec.agg !== "none") {
    const names = { sum: "合計", avg: "平均", min: "最小", max: "最大" };
    return `${names[spec.agg] || spec.agg}(${spec.y})`;
  }
  return spec.y || "";
}

function renderChart(canvas, spec, result) {
  fitCanvasToFacets(canvas, spec, result);
  const { ctx, w, h } = setupCanvas(canvas);
  const reg = CHART_REGISTRY.get(spec.chart_type);
  const twoD = !!(spec.facet && spec.facet2);
  if (reg) {
    // form.facet を宣言した登録チャートはファセット分割に乗せる
    if (spec.facet && reg.form && reg.form.facet) {
      if (Array.isArray(result.groups)) {
        // fetch チャート: 分割はサーバー側(/api/analyze/group)で済んでいる。
        // グループごとの成否がそのままパネルの成否になる(SPC管理図)
        if (twoD && result.group2) {
          renderFetchFacetGrid(ctx, w, h, spec, reg, result);
          return;
        }
        const notes = result.truncated
          ? [`${result.group}: ${result.total}件中 先頭${result.shown}件を表示`]
          : [];
        renderRegistryFacets(
          ctx, w, h, spec, reg,
          result.groups.map((g) => ({ name: `${result.group} = ${g.value}`, result: g.result, error: g.error })),
          { allResults: result.groups.filter((g) => g.result).map((g) => g.result) },
          notes,
        );
        return;
      }
      const fciReg = result.columns.indexOf("f");
      if (fciReg >= 0) {
        if (twoD && result.columns.indexOf("f2") >= 0) {
          renderSqlFacetGrid(ctx, w, h, spec, result, reg);
          return;
        }
        renderFacets(ctx, w, h, spec, result, fciReg, reg);
        return;
      }
    }
    reg.render(ctx, w, h, spec, result, CHART_HELPERS);
    return;
  }
  if (!["bar", "line", "scatter", "histogram"].includes(spec.chart_type)) {
    // 未登録タイプ: プラグイン未導入の環境でプロジェクトを開いた場合など
    // (docs/plugin-api-draft.md 6.1「壊さない・原因を示す」)
    ctx.fillStyle = CHART_COLORS.text;
    ctx.fillText(`未対応のチャートタイプです: ${spec.chart_type}(プラグインが必要な可能性があります)`, 16, 24);
    return;
  }
  // ファセット分割(f列があれば、値ごとに小さなチャートを格子状に描く)
  const fci = result.columns.indexOf("f");
  if (spec.facet && fci >= 0) {
    if (twoD && result.columns.indexOf("f2") >= 0) {
      renderSqlFacetGrid(ctx, w, h, spec, result, null);
      return;
    }
    renderFacets(ctx, w, h, spec, result, fci, null);
    return;
  }
  drawChartArea(ctx, w, h, spec, result, null, null);
}

/** ファセットの各パネル上部に確保するタイトル帯の高さ */
const FACET_TITLE_H = 16;
/** 格子の外周に確保する余白(Canvas端で文字が切れないように) */
const FACET_PAD = 4;
/** 段の間隔(上段の最下部の文字と下段の見出しが接触しないように) */
const FACET_ROW_GAP = 8;

/** 登録チャートの render に渡す前に、文字の描画状態を既定へ戻す。
 *  ファセットの見出しを描いた状態(textBaseline="top" 等)が残っていると、
 *  基準線を自分で設定していないチャートの文字が下へずれて枠外に出る
 *  (SPC管理図で実際に発生。情報行が下段の見出しと重なり、最下段は見切れた)。 */
function resetTextState(ctx) {
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
}

/** 2次元ファセットで各軸に表示するカテゴリ数の上限(6×6=36セルまで) */
const FACET2_DIM_MAX = 6;

/** ファセットのセル間で共有するスケールを求める(1次元・2次元で共通)。
 *  cellArrays: 各セルの行配列の配列(ヒストグラムの度数軸を揃えるのに使う)。
 *  Y軸レンジ・ヒストグラムのビン区切りと度数軸・系列の色順をセル間で揃える
 *  (揃えないとパネル同士を見比べられないため)。
 *  戻り値: {shared, specShared, notes}。登録チャートのスケール共有は
 *  render 側が shared.allRows / allResults から行うので、ここでは扱わない。 */
function computeFacetShared(spec, result, reg, cellArrays) {
  const shared = {};
  const specShared = { ...spec };
  const notes = [];
  const xi = result.columns.indexOf("x");
  // 系列の色と凡例の並びをセル間で固定する
  // (セルごとに出現順で色を割ると、同じ系列が別の色になってしまう)
  const si = reg ? -1 : result.columns.indexOf("s");
  if (si >= 0) {
    const order = [];
    for (const r of result.rows) {
      const n = r[si] === null ? "(null)" : String(r[si]);
      if (!order.includes(n)) order.push(n);
    }
    if (order.length > 8) notes.push(`${order.length}系列中 先頭8系列を表示`);
    shared.seriesOrder = order.slice(0, 8);
  }
  if (reg) {
    // 登録チャートのスケール共有は render 側が行う
  } else if (spec.chart_type === "histogram") {
    const vals = result.rows.map((r) => Number(r[xi])).filter((v) => isFinite(v));
    if (vals.length) {
      const lo = Math.min(...vals);
      const hi = Math.max(...vals);
      shared.histRange = [lo, hi]; // ビンの区切りも全セルで共有
      if (numOrNull(spec.y_min) === null && numOrNull(spec.y_max) === null) {
        // 共有ビンで各セルの最大度数を求め、度数軸も揃える
        const nb = Math.max(2, Math.min(200, spec.bins || 20));
        const width = hi - lo || 1;
        let peak = 1;
        for (const rows of cellArrays) {
          const counts = new Array(nb).fill(0);
          for (const r of rows) {
            const v = Number(r[xi]);
            if (!isFinite(v)) continue;
            let b = Math.floor(((v - lo) / width) * nb);
            if (b >= nb) b = nb - 1;
            peak = Math.max(peak, ++counts[b]);
          }
        }
        const t = niceTicks(0, peak, 5);
        specShared.y_min = String(t[0]);
        specShared.y_max = String(t[t.length - 1]);
      }
    }
  } else if (numOrNull(spec.y_min) === null && numOrNull(spec.y_max) === null) {
    const yi = result.columns.indexOf("y");
    let gmin = Infinity;
    let gmax = -Infinity;
    if (yi >= 0) {
      for (const r of result.rows) {
        const v = Number(r[yi]);
        if (isFinite(v)) {
          gmin = Math.min(gmin, v);
          gmax = Math.max(gmax, v);
        }
      }
    }
    if (isFinite(gmin)) {
      if (spec.chart_type === "bar") {
        gmin = Math.min(0, gmin); // 棒グラフは0基点を維持する
        gmax = Math.max(0, gmax);
      }
      const t = niceTicks(gmin, gmax, 5);
      specShared.y_min = String(t[0]);
      specShared.y_max = String(t[t.length - 1]);
    }
  }
  return { shared, specShared, notes };
}

/** 空のセル(その行×列の組にデータがない)を控えめに示す */
function emptyFacetCell(ctx, cw, chh) {
  ctx.fillStyle = CHART_COLORS.muted;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("データなし", cw / 2, chh / 2);
}

/** 短縮表示 */
function ellipsize(s, n) {
  return s.length > n ? s.slice(0, n) + "…" : s;
}

/** 2次元ファセット(行×列)の格子レイアウト。列見出しを上、行見出しを左に置き、
 *  同じ列は縦に、同じ行は横に揃うので2つのカテゴリを同時に見比べられる。
 *  各セルは drawCell(ctx, cw, chh, colKey, rowKey) が描く。 */
function renderFacetGrid(ctx, w, h, colKeys, rowKeys, colVar, rowVar, drawCell, opts) {
  opts = opts || {};
  const legend = opts.legend;
  const legendW = legend ? legend.width : 0;
  const ncols = colKeys.length, nrows = rowKeys.length;
  if (!ncols || !nrows) {
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
    return;
  }
  // 左の行見出し帯の幅は最長ラベルから決める(40〜120)
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  const rowLabelW = Math.min(120, Math.max(40, ...rowKeys.map((k) => ctx.measureText(ellipsize(k, 16)).width + 10)));
  const CAP_H = 15; // 「列=… × 行=…」のキャプション帯
  const HEAD_H = FACET_TITLE_H; // 列見出し帯
  const topStrip = CAP_H + HEAD_H;
  const gridX = FACET_PAD + rowLabelW;
  const gridY = topStrip + FACET_PAD;
  const gridW = w - gridX - legendW - FACET_PAD;
  const gridH = h - gridY - FACET_PAD;
  const cw = gridW / ncols;
  const chh = (gridH - FACET_ROW_GAP * (nrows - 1)) / nrows;

  // どちらの列がどの軸かをキャプションで明示(見出しは値だけにして簡潔に)
  ctx.fillStyle = CHART_COLORS.muted;
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
  ctx.fillText(`列(横)= ${ellipsize(colVar, 16)}   行(縦)= ${ellipsize(rowVar, 16)}`, 2, 1);
  if (opts.notes && opts.notes.length) {
    ctx.textAlign = "right";
    ctx.fillText(opts.notes.join(" / "), w - legendW - 2, 1);
  }

  // 列見出し(上)
  ctx.fillStyle = CHART_COLORS.text;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  colKeys.forEach((c, ci) => ctx.fillText(ellipsize(c, 16), gridX + (ci + 0.5) * cw, CAP_H + 1));

  // 行見出し(左) + 各セル
  rowKeys.forEach((rk, ri) => {
    const cy = gridY + ri * (chh + FACET_ROW_GAP);
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText(ellipsize(rk, 16), 2, cy + chh / 2);
    colKeys.forEach((ck, ci) => {
      ctx.save();
      ctx.translate(gridX + ci * cw, cy);
      ctx.beginPath();
      ctx.rect(0, 0, cw, chh);
      ctx.clip();
      resetTextState(ctx);
      drawCell(ctx, cw, chh, ck, rk);
      ctx.restore();
    });
  });

  // 凡例(格子の外側右。最上段の高さに合わせる)
  if (legend) {
    ctx.save();
    ctx.translate(w - legendW, gridY);
    resetTextState(ctx);
    legend.draw(ctx, legendW, chh);
    ctx.restore();
  }
}

/** 2つの値を格子のキーに合成する(区切りに制御文字を使い衝突を避ける) */
function cellKey(col, row) {
  return `${col} ${row}`;
}

/** 上限を超えたカテゴリを先頭 max 件に絞り、必要なら注記を返す */
function capDim(keys, max, varName) {
  if (keys.length <= max) return { keys, note: null };
  return { keys: keys.slice(0, max), note: `${varName}: ${keys.length}件中 先頭${max}件を表示` };
}

/** SQL派生(f/f2列)の2次元ファセット。組み込みチャートとウェハーマップで共通。 */
function renderSqlFacetGrid(ctx, w, h, spec, result, reg) {
  const fi = result.columns.indexOf("f");
  const f2i = result.columns.indexOf("f2");
  const colKeys = [], rowKeys = [], byCell = new Map();
  for (const r of result.rows) {
    const col = r[fi] === null ? "(null)" : String(r[fi]);
    const row = r[f2i] === null ? "(null)" : String(r[f2i]);
    if (!colKeys.includes(col)) colKeys.push(col);
    if (!rowKeys.includes(row)) rowKeys.push(row);
    const k = cellKey(col, row);
    if (!byCell.has(k)) byCell.set(k, []);
    byCell.get(k).push(r);
  }
  const cCap = capDim(colKeys, FACET2_DIM_MAX, spec.facet);
  const rCap = capDim(rowKeys, FACET2_DIM_MAX, spec.facet2);
  const notes = [cCap.note, rCap.note].filter(Boolean);
  const cellArrays = [];
  for (const c of cCap.keys) for (const r of rCap.keys) {
    const a = byCell.get(cellKey(c, r));
    if (a) cellArrays.push(a);
  }
  const { shared, specShared, notes: sn } = computeFacetShared(spec, result, reg, cellArrays);
  notes.push(...sn);
  const drawCell = (ctx, cw, chh, col, row) => {
    const rows = byCell.get(cellKey(col, row));
    if (!rows) return emptyFacetCell(ctx, cw, chh);
    if (reg) {
      reg.render(ctx, cw, chh, specShared, { columns: result.columns, rows }, CHART_HELPERS, {
        facetValue: `${col} / ${row}`,
        allRows: result.rows,
      });
    } else {
      drawChartArea(ctx, cw, chh, specShared, { columns: result.columns, rows }, null, shared);
    }
  };
  const legend = reg && reg.renderLegend
    ? { width: reg.legendWidth || 76, draw: (c, lw, lh) => reg.renderLegend(c, lw, lh, specShared, result, CHART_HELPERS) }
    : null;
  renderFacetGrid(ctx, w, h, cCap.keys, rCap.keys, spec.facet, spec.facet2, drawCell, { legend, notes });
}

/** fetch派生(グループ別実行API)の2次元ファセット。SPC管理図が利用。 */
function renderFetchFacetGrid(ctx, w, h, spec, reg, result) {
  const colKeys = [], rowKeys = [], byCell = new Map();
  for (const g of result.groups) {
    const col = String(g.value), row = String(g.value2);
    if (!colKeys.includes(col)) colKeys.push(col);
    if (!rowKeys.includes(row)) rowKeys.push(row);
    byCell.set(cellKey(col, row), g);
  }
  const cCap = capDim(colKeys, FACET2_DIM_MAX, result.group);
  const rCap = capDim(rowKeys, FACET2_DIM_MAX, result.group2);
  const notes = [cCap.note, rCap.note].filter(Boolean);
  if (result.truncated) {
    notes.push(`${result.group}×${result.group2}: ${result.total}組中 先頭${result.shown}組を実行`);
  }
  const allResults = result.groups.filter((g) => g.result).map((g) => g.result);
  const drawCell = (ctx, cw, chh, col, row) => {
    const g = byCell.get(cellKey(col, row));
    if (!g) return emptyFacetCell(ctx, cw, chh);
    if (g.error) {
      ctx.fillStyle = CHART_COLORS.danger;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      wrapText(ctx, g.error, cw / 2, chh / 2, cw - 12, 14);
      return;
    }
    reg.render(ctx, cw, chh, spec, g.result, CHART_HELPERS, { facetValue: `${col} / ${row}`, allResults });
  };
  renderFacetGrid(ctx, w, h, cCap.keys, rCap.keys, result.group, result.group2, drawCell, { notes });
}

/** ファセット表示(seabornのFacetGrid相当)。指定列の値ごとにデータを分け、
 *  同じCanvas内に小さなチャートを格子状に描く。
 *  Y軸レンジ・ヒストグラムのビン・系列の色はセル間で共有する
 *  (揃えないとパネル同士を見比べられないため)。 */
function renderFacets(ctx, w, h, spec, result, fi, reg) {
  // 登録チャートは facetMax で上限を広げられる(ウェハーマップは25枚=1ロット分)
  const MAX_FACETS = (reg && reg.form && reg.form.facetMax) || 12;
  const names = [];
  const byFacet = new Map();
  for (const r of result.rows) {
    const k = r[fi] === null ? "(null)" : String(r[fi]);
    if (!byFacet.has(k)) {
      byFacet.set(k, []);
      names.push(k);
    }
    byFacet.get(k).push(r);
  }
  if (!names.length) {
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
    return;
  }
  const notes = [];
  let shown = names;
  if (names.length > MAX_FACETS) {
    shown = names.slice(0, MAX_FACETS);
    notes.push(`${spec.facet}: ${names.length}件中 先頭${MAX_FACETS}件を表示`);
  }

  const cellArrays = shown.map((name) => byFacet.get(name));
  const { shared, specShared, notes: sharedNotes } = computeFacetShared(spec, result, reg, cellArrays);
  notes.push(...sharedNotes);

  if (reg) {
    // 登録チャートは共通のパネル描画に委譲する(スケール共有は render 側が
    // shared.allRows から行う)
    renderRegistryFacets(
      ctx, w, h, specShared, reg,
      shown.map((name) => ({ name, result: { columns: result.columns, rows: byFacet.get(name) } })),
      { allRows: result.rows, legendResult: result },
      notes,
    );
    return;
  }

  // 格子レイアウト(件数から列数を決める)
  const n = shown.length;
  const cols = facetCols(n);
  const gridRows = Math.ceil(n / cols);
  const cw = w / cols;
  // 登録チャートと同じく、外周の余白と段の間隔を引いてから高さを決める
  const chh = (h - FACET_PAD * 2 - FACET_ROW_GAP * (gridRows - 1)) / gridRows;
  shown.forEach((name, i) => {
    ctx.save();
    ctx.translate((i % cols) * cw, FACET_PAD + Math.floor(i / cols) * (chh + FACET_ROW_GAP));
    ctx.beginPath();
    ctx.rect(0, 0, cw, chh);
    ctx.clip();
    drawChartArea(ctx, cw, chh, specShared, { columns: result.columns, rows: byFacet.get(name) }, name, shared);
    ctx.restore();
  });
  if (notes.length) {
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "right";
    ctx.textBaseline = "top";
    ctx.fillText(notes.join(" / "), w - 4, 2);
  }
}

/** パネル数から格子の列数を決める(25枚=1ロットが5×5に収まるように) */
function facetCols(n) {
  return n <= 2 ? n : n <= 4 ? 2 : n <= 9 ? 3 : n <= 16 ? 4 : 5;
}

/** 登録チャートのファセット描画。1パネル分の結果は呼び出し側が用意する
 *  (SQLチャートは f 列で分割、fetch チャートはサーバーのグループ別実行の
 *  結果をそのまま使う)。パネル間のスケール共有は shared 経由で render 側が行う。 */
function renderRegistryFacets(ctx, w, h, spec, reg, panels, shared, notes) {
  if (!panels.length) {
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
    return;
  }
  const cols = facetCols(panels.length);
  const gridRows = Math.ceil(panels.length / cols);
  // 凡例(カラースケール等)を持つ登録チャートは、格子の外側右に領域を確保する。
  // パネル内に描くと1枚だけ余白が変わり、大きさと位置が揃わなくなる
  const legendW = reg.renderLegend && shared.legendResult ? reg.legendWidth || 76 : 0;
  const gw = w - legendW;
  const cw = gw / cols;
  // 外周の余白と段の間隔を先に引いてからパネル高さを決める。これがないと
  // 最上段の見出しと最下段の情報行がCanvas端で切れ、段の境目でも文字が接触する
  const chh = (h - FACET_PAD * 2 - FACET_ROW_GAP * (gridRows - 1)) / gridRows;
  panels.forEach((p, i) => {
    ctx.save();
    ctx.translate((i % cols) * cw, FACET_PAD + Math.floor(i / cols) * (chh + FACET_ROW_GAP));
    ctx.beginPath();
    ctx.rect(0, 0, cw, chh);
    ctx.clip();
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.fillText(p.name.length > 24 ? p.name.slice(0, 24) + "…" : p.name, cw / 2, 0);
    ctx.translate(0, FACET_TITLE_H);
    if (p.error) {
      // 1パネルの失敗で他を消さない(サーバー側も止めずに返している)
      ctx.fillStyle = CHART_COLORS.danger;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      wrapText(ctx, p.error, cw / 2, (chh - FACET_TITLE_H) / 2, cw - 16, 14);
    } else {
      resetTextState(ctx);
      reg.render(ctx, cw, chh - FACET_TITLE_H, spec, p.result, CHART_HELPERS, {
        facetValue: p.name,
        ...shared,
      });
    }
    ctx.restore();
  });
  if (legendW) {
    // 凡例は最上段の右端パネルの右隣(格子の外側)に置く
    ctx.save();
    ctx.translate(gw, FACET_PAD + FACET_TITLE_H);
    resetTextState(ctx);
    reg.renderLegend(ctx, legendW, chh - FACET_TITLE_H, spec, shared.legendResult, CHART_HELPERS);
    ctx.restore();
  }
  if (notes && notes.length) {
    ctx.fillStyle = CHART_COLORS.text;
    ctx.textAlign = "right";
    ctx.textBaseline = "top";
    ctx.fillText(notes.join(" / "), gw - 4, 2);
  }
}

/** Canvasに折り返しでテキストを描く(パネル内のエラー表示用) */
function wrapText(ctx, text, cx, cy, maxW, lineH) {
  const lines = [];
  let cur = "";
  for (const ch of text) {
    if (ctx.measureText(cur + ch).width > maxW && cur) {
      lines.push(cur);
      cur = ch;
    } else {
      cur += ch;
    }
  }
  if (cur) lines.push(cur);
  const top = cy - ((lines.length - 1) * lineH) / 2;
  lines.forEach((l, i) => ctx.fillText(l, cx, top + i * lineH));
}

/** 1つのチャートを (0,0)〜(w,h) に描く。title はファセット名(ファセット時のみ)、
 *  shared はファセット間で共有する設定(系列の並び・ヒストグラムのビン範囲)。 */
function drawChartArea(ctx, w, h, spec, result, title, shared) {
  const xi = result.columns.indexOf("x");
  const yi = result.columns.indexOf("y");
  const si = result.columns.indexOf("s");
  const C = CHART_COLORS;

  // 系列分解(s列がなければ全行を単一系列として扱う)
  const MAX_SERIES = 8;
  const notes = [];
  let seriesNames = [];
  const bySeries = new Map();
  if (si >= 0 && shared && shared.seriesOrder) {
    // ファセット間で系列の色と凡例を揃える
    seriesNames = shared.seriesOrder;
    for (const n of seriesNames) bySeries.set(n, []);
    for (const r of result.rows) {
      const name = r[si] === null ? "(null)" : String(r[si]);
      if (bySeries.has(name)) bySeries.get(name).push(r);
    }
  } else if (si >= 0) {
    for (const r of result.rows) {
      const name = r[si] === null ? "(null)" : String(r[si]);
      if (!bySeries.has(name)) {
        bySeries.set(name, []);
        seriesNames.push(name);
      }
      bySeries.get(name).push(r);
    }
    if (seriesNames.length > MAX_SERIES) {
      notes.push(`${seriesNames.length}系列中 先頭${MAX_SERIES}系列を表示`);
      seriesNames = seriesNames.slice(0, MAX_SERIES);
    }
  } else {
    seriesNames = [""];
    bySeries.set("", result.rows);
  }
  const hasLegend = si >= 0 && seriesNames.length > 1;
  const color = (k) => (si >= 0 ? SERIES_COLORS[k % SERIES_COLORS.length] : C.accent);

  const yLabel = chartYLabel(spec);
  const tOff = title ? 16 : 0; // ファセット名の分だけ上に余白を足す
  const m = { l: 58 + (yLabel ? 16 : 0), r: 16, t: (hasLegend ? 34 : 14) + tOff, b: 52 };
  const pw = w - m.l - m.r, ph = h - m.t - m.b;

  const drawYLabel = () => {
    if (!yLabel) return;
    ctx.save();
    ctx.fillStyle = C.text;
    ctx.translate(14, m.t + ph / 2);
    ctx.rotate(-Math.PI / 2);
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(yLabel.length > 24 ? yLabel.slice(0, 24) + "…" : yLabel, 0, 0);
    ctx.restore();
  };

  const drawTitle = () => {
    if (!title) return;
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    const tl = title.length > 24 ? title.slice(0, 24) + "…" : title;
    ctx.fillText(tl, w / 2, 3);
  };

  const drawLegend = () => {
    if (!hasLegend) return;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    let lx = m.l;
    for (let k = 0; k < seriesNames.length; k++) {
      const lb = seriesNames[k].length > 16 ? seriesNames[k].slice(0, 16) + "…" : seriesNames[k];
      const need = 14 + ctx.measureText(lb).width + 16;
      if (lx + need > w - m.r) break; // 幅に収まらない分は省略
      ctx.fillStyle = color(k);
      ctx.fillRect(lx, 10 + tOff, 10, 10);
      ctx.fillStyle = C.text;
      ctx.fillText(lb, lx + 14, 15 + tOff);
      lx += need;
    }
  };

  const drawNotes = () => {
    if (!notes.length) return;
    ctx.fillStyle = C.text;
    ctx.textAlign = "right";
    ctx.textBaseline = "top";
    ctx.fillText(notes.join(" / "), w - m.r, (hasLegend ? 24 : 2) + tOff);
  };

  const drawAxes = (yTicks, yMin, yMax) => {
    ctx.strokeStyle = C.grid;
    ctx.fillStyle = C.text;
    ctx.lineWidth = 1;
    for (const t of yTicks) {
      const yy = m.t + ph - ((t - yMin) / (yMax - yMin)) * ph;
      ctx.beginPath();
      ctx.moveTo(m.l, yy);
      ctx.lineTo(w - m.r, yy);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.textBaseline = "middle";
      ctx.fillText(fmtTick(t), m.l - 6, yy);
    }
  };

  /** データの描画はプロット領域でクリップする。軸レンジの計算に想定外があっても
   *  軸ラベルの上に描かれないための保険(手動レンジ指定時にも効く) */
  const clipPlot = (draw) => {
    ctx.save();
    ctx.beginPath();
    ctx.rect(m.l, m.t, pw, ph);
    ctx.clip();
    draw();
    ctx.restore();
  };

  const noData = () => {
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
  };

  drawTitle();

  if (spec.chart_type === "histogram") {
    // 系列(色分け)対応: ビンの区切りは全系列で共有する。
    // 系列ごとに区切りを変えると分布の形を比較できないため、必ず全体の範囲から作る
    const seriesVals = seriesNames.map((n) =>
      bySeries
        .get(n)
        .map((r) => Number(r[xi]))
        .filter((v) => isFinite(v))
    );
    const vals = seriesVals.flat();
    if (!vals.length) return noData();
    // ファセット表示ではビンの区切りを全セルで共有する
    let lo = shared && shared.histRange ? shared.histRange[0] : Math.min(...vals);
    let hi = shared && shared.histRange ? shared.histRange[1] : Math.max(...vals);
    // X軸の手動レンジ(空欄なら自動)。指定時はその範囲だけをビン化して表示する
    const mlo = numOrNull(spec.x_min), mhi = numOrNull(spec.x_max);
    const nlo = mlo !== null ? mlo : lo, nhi = mhi !== null ? mhi : hi;
    if (nhi > nlo) { lo = nlo; hi = nhi; }
    const nb = Math.max(2, Math.min(200, spec.bins || 20));
    const width = (hi - lo) || 1;
    const countsPer = seriesVals.map((vs) => {
      const counts = new Array(nb).fill(0);
      for (const v of vs) {
        if (v < lo || v > hi) continue; // 手動レンジ外は端のビンを膨らませないよう除外
        let b = Math.floor(((v - lo) / width) * nb);
        if (b >= nb) b = nb - 1;
        counts[b]++;
      }
      return counts;
    });
    const yPeak = Math.max(...countsPer.map((c) => Math.max(...c)));
    const hRange = applyManualRange(spec, niceTicks(0, yPeak, 5), 5);
    drawAxes(hRange.ticks, hRange.min, hRange.max);
    const hpy = (v) => m.t + ph - ((v - hRange.min) / ((hRange.max - hRange.min) || 1)) * ph;
    clipPlot(() => {
      const overlay = seriesNames.length > 1;
      countsPer.forEach((counts, k) => {
        ctx.fillStyle = color(k);
        // 複数系列は半透明で重ねる(重なりが色の濃さで見える)
        ctx.globalAlpha = overlay ? 0.55 : 1;
        for (let b = 0; b < nb; b++) {
          if (!counts[b]) continue;
          const bx = m.l + (b / nb) * pw;
          const bw = pw / nb - 1;
          // 棒は軸の下端から度数の高さまで(手動レンジで下端が0でなくても崩れない)
          const top = hpy(counts[b]);
          ctx.fillRect(bx, top, Math.max(1, bw), m.t + ph - top);
        }
      });
      ctx.globalAlpha = 1;
    });
    // X軸ラベル
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    const xTicks = niceTicks(lo, hi, 6);
    for (const t of xTicks) {
      const xx = m.l + ((t - lo) / width) * pw;
      if (xx >= m.l - 1 && xx <= w - m.r + 1) ctx.fillText(fmtTick(t), xx, m.t + ph + 6);
    }
    ctx.fillText(spec.x, m.l + pw / 2, h - 16);
    drawYLabel();
    drawLegend();
    drawNotes();
    return;
  }

  if (xi < 0 || yi < 0 || !result.rows.length) return noData();

  if (spec.chart_type === "scatter" || spec.chart_type === "line") {
    const allRows = seriesNames.flatMap((n) => bySeries.get(n)).filter((r) => r[yi] !== null);
    if (!allRows.length) return noData();
    const xNumeric = allRows.every((r) => typeof r[xi] === "number");
    // カテゴリXは全系列共通の出現順インデックスに揃える
    const catIndex = new Map();
    const catLabels = [];
    if (!xNumeric) {
      for (const r of allRows) {
        const cx = String(r[xi]);
        if (!catIndex.has(cx)) {
          catIndex.set(cx, catIndex.size);
          catLabels.push(cx);
        }
      }
    }
    const seriesPts = seriesNames.map((n) =>
      bySeries
        .get(n)
        .filter((r) => r[yi] !== null)
        .map((r) => [xNumeric ? Number(r[xi]) : catIndex.get(String(r[xi])), Number(r[yi])])
        .filter((p) => isFinite(p[0]) && isFinite(p[1]))
        .sort((a, b) => a[0] - b[0])
    );
    const flat = seriesPts.flat();
    if (!flat.length) return noData();
    const xs = flat.map((p) => p[0]), ys = flat.map((p) => p[1]);
    // 散布図・折れ線は0を強制しない(歩留まり86〜96%を0〜100で描くと変化が読めない)。
    // 棒グラフは長さが値を表すため0を含める(下の分岐)。
    const yRange = applyManualRange(spec, niceTicks(Math.min(...ys), Math.max(...ys), 5), 5);
    const yTicks = yRange.ticks;
    const yMin = yRange.min, yMax = yRange.max;
    // X軸の手動レンジはX軸が数値のときのみ(カテゴリ軸はインデックス配置のため無効)
    const xRange = xNumeric
      ? applyManualRange(spec, niceTicks(Math.min(...xs), Math.max(...xs), 6), 6, "x")
      : { min: Math.min(...xs), max: Math.max(...xs) };
    const xMin = xRange.min, xMax = xRange.max;
    drawAxes(yTicks, yMin, yMax);
    const px = (x) => m.l + ((x - xMin) / ((xMax - xMin) || 1)) * pw;
    const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
    const rad = spec.chart_type === "line" ? (flat.length > 200 ? 0 : 2.5) : Math.max(1.5, 4 - Math.log10(flat.length + 1));
    clipPlot(() => seriesPts.forEach((pts, k) => {
      const col = color(k);
      if (spec.chart_type === "line") {
        ctx.strokeStyle = col;
        ctx.lineWidth = 1.6;
        ctx.beginPath();
        pts.forEach((p, i) => (i ? ctx.lineTo(px(p[0]), py(p[1])) : ctx.moveTo(px(p[0]), py(p[1]))));
        ctx.stroke();
      }
      if (rad > 0) {
        ctx.fillStyle = col;
        ctx.globalAlpha = spec.chart_type === "line" ? 1 : 0.65;
        for (const p of pts) {
          ctx.beginPath();
          ctx.arc(px(p[0]), py(p[1]), rad, 0, Math.PI * 2);
          ctx.fill();
        }
        ctx.globalAlpha = 1;
      }
    }));
    // X軸
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    if (xNumeric) {
      // 手動レンジ指定時は ticksWithin 由来の目盛りを使う(端まで覆う)
      for (const t of (xRange.ticks || niceTicks(xMin, xMax, 6))) {
        const xx = px(t);
        if (xx >= m.l - 1 && xx <= w - m.r + 1) ctx.fillText(fmtTick(t), xx, m.t + ph + 6);
      }
    } else {
      // ラベル幅から重ならずに置ける個数を求めて間引く
      const shown = catLabels.map((lb) => (lb.length > 12 ? lb.slice(0, 12) + "…" : lb));
      const wMax = Math.max(...shown.map((lb) => ctx.measureText(lb).width), 1);
      const maxFit = Math.max(1, Math.floor(pw / (wMax + 12)));
      const stepN = Math.ceil(catLabels.length / maxFit);
      shown.forEach((lb, i) => {
        if (i % stepN === 0) ctx.fillText(lb, px(i), m.t + ph + 6);
      });
    }
    ctx.fillText(spec.x, m.l + pw / 2, h - 16);
    drawYLabel();
    drawLegend();
    drawNotes();
    return;
  }

  // 棒グラフ(カテゴリ × 系列 → グループ棒)
  const MAX_BARS = 60;
  // カテゴリ軸は全系列共通(クエリのORDER BY x順で採番)
  let cats = [];
  const seen = new Set();
  for (const r of result.rows) {
    if (r[yi] === null) continue;
    const cx = String(r[xi]);
    if (!seen.has(cx)) {
      seen.add(cx);
      cats.push(cx);
    }
  }
  if (!cats.length) return noData();
  if (cats.length > MAX_BARS) {
    notes.push(`${cats.length}カテゴリ中 先頭${MAX_BARS}件を表示`);
    cats = cats.slice(0, MAX_BARS);
  }
  const catPos = new Map(cats.map((c, i) => [c, i]));
  // 系列ごとの カテゴリ→値 表(同一キー重複は後勝ち)
  const vals = seriesNames.map((n) => {
    const mp = new Map();
    for (const r of bySeries.get(n)) {
      if (r[yi] === null) continue;
      const v = Number(r[yi]);
      if (isFinite(v) && catPos.has(String(r[xi]))) mp.set(String(r[xi]), v);
    }
    return mp;
  });
  const allVals = vals.flatMap((mp) => [...mp.values()]);
  if (!allVals.length) return noData();
  const barRange = applyManualRange(
    spec,
    niceTicks(Math.min(0, Math.min(...allVals)), Math.max(0, Math.max(...allVals)), 5),
    5
  );
  const yTicks = barRange.ticks;
  const yMin = barRange.min, yMax = barRange.max;
  drawAxes(yTicks, yMin, yMax);
  const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
  const groupW = pw / cats.length;
  const inner = groupW * 0.76;
  const barW = inner / seriesNames.length;
  clipPlot(() => seriesNames.forEach((n, k) => {
    ctx.fillStyle = color(k);
    const mp = vals[k];
    cats.forEach((c, i) => {
      if (!mp.has(c)) return;
      const v = mp.get(c);
      const x0 = m.l + i * groupW + groupW * 0.12 + k * barW;
      const y0 = py(Math.max(0, v));
      const hh = Math.abs(py(v) - py(0));
      const bw = Math.max(1, barW - (seriesNames.length > 1 ? 1 : 0));
      ctx.fillRect(x0, v >= 0 ? y0 : py(0), bw, Math.max(1, hh));
    });
  }));
  // カテゴリラベル
  ctx.fillStyle = C.text;
  const rotate = cats.length > 8 || cats.some((c) => c.length > 6);
  const short = (c) => (c.length > 14 ? c.slice(0, 14) + "…" : c);
  // 間引く本数はカテゴリ数ではなく「実際に確保できる横幅」から決める。
  // 固定本数だと、ファセットで狭くなったパネルでラベルが重なって潰れる
  // (斜めラベルは文字高さ÷sin(36°)ぶんの横間隔が要る)
  const pitch = rotate
    ? 19
    : Math.max(...cats.map((c) => ctx.measureText(short(c)).width)) + 10;
  const stepN = Math.max(1, Math.ceil(pitch / groupW));
  cats.forEach((c, i) => {
    const cx = m.l + i * groupW + groupW / 2;
    const label = short(c);
    if (i % stepN !== 0) return;
    ctx.save();
    ctx.translate(cx, m.t + ph + 6);
    if (rotate) {
      ctx.rotate(-Math.PI / 5);
      ctx.textAlign = "right";
      ctx.textBaseline = "top";
    } else {
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
    }
    ctx.fillText(label, 0, 0);
    ctx.restore();
  });
  drawYLabel();
  drawLegend();
  drawNotes();
}

// ---------- 分析 ----------

const CLUSTER_COLORS = SERIES_COLORS; // クラスタ色は系列色と同じパレットを使う
let anColumns = []; // {name, numeric}
let lastCluster = null;
let lastClusterReq = null;

// ---------- チャートタイプ登録(Plugin API 検証実装) ----------
// 新しいチャートタイプは registerChartType で登録する。将来のチャートプラグイン
// (docs/plugin-api-draft.md 6章)と同一のAPIであり、ウェハーマップは本体組み込みの
// ままこのAPIの最初の利用者としてドラフトの実用性を検証する(ドラフト8章の折衷案)。
// 既存5種(棒/折れ線/散布図/ヒストグラム/テーブル)は従来の分岐実装のまま。

const CHART_REGISTRY = new Map();
const kohaku = {
  /**
   * def: {
   *   type, label,                       // ChartSpec.chart_type の識別子と表示名
   *   form: {x, y, value, series, agg},  // 使うフォーム行(文字列を渡すとラベルを差し替え)
   *   buildQuery(spec, base),            // データ取得SQL(base はフィルタ適用済みソース)
   *   render(ctx, w, h, spec, result, helpers), // Canvas 2D 描画
   * }
   */
  registerChartType(def) {
    CHART_REGISTRY.set(def.type, def);
  },
};

/** 登録チャートへ渡す描画ユーティリティ(本体チャートと見た目を揃えるため) */
const CHART_HELPERS = {
  niceTicks,
  ticksWithin,
  applyManualRange,
  fmtTick,
  colors: CHART_COLORS,
  seriesColors: SERIES_COLORS,
};

// ---------- ウェハーマップ(v0.4) ----------
// ダイ座標(x, y)ごとに値を集計し、色分けした格子+ウェハー外周円として描画する

/** 低=赤 / 中=黄 / 高=緑 の3点補間(歩留まりの直感に合わせる) */
function waferColor(v, vMin, vMax) {
  const lerp = (a, b, t) => Math.round(a + (b - a) * t);
  const t = (v - vMin) / (vMax - vMin);
  const [c0, c1, u] =
    t < 0.5 ? [[224, 108, 117], [224, 208, 92], t * 2] : [[224, 208, 92], [88, 201, 164], t * 2 - 1];
  return `rgb(${lerp(c0[0], c1[0], u)}, ${lerp(c0[1], c1[1], u)}, ${lerp(c0[2], c1[2], u)})`;
}

/** カラースケール(縦バー+上下の値ラベル)を (sx, sy) から高さ sh で描く */
function drawWaferScale(ctx, sx, sy, sh, vMin, vMax, H) {
  for (let i = 0; i < sh; i++) {
    ctx.fillStyle = waferColor(vMax - ((vMax - vMin) * i) / sh, vMin, vMax);
    ctx.fillRect(sx, sy + i, 14, 1);
  }
  ctx.fillStyle = H.colors.text;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.fillText(H.fmtTick(vMax), sx + 18, sy + 8);
  ctx.fillText(H.fmtTick(vMin), sx + 18, sy + sh);
}

/** ウェハーマップの値域(ファセット時は全ウェハー共通) */
function waferRange(rows, vi) {
  const vs = rows.map((r) => r[vi]).filter((v) => v !== null);
  let vMin = Math.min(...vs);
  let vMax = Math.max(...vs);
  if (vMin === vMax) {
    vMin -= 1;
    vMax += 1;
  }
  return [vMin, vMax];
}

kohaku.registerChartType({
  type: "wafermap",
  label: "ウェハーマップ",
  form: { x: "X座標(ダイ)", y: "Y座標(ダイ)", value: "値(歩留まり等)", agg: true, facet: true, facetMax: 25 },
  buildQuery(spec, base) {
    if (!spec.value && spec.agg !== "count") {
      throw new Error("値の列を指定してください(件数を数える場合は集計=件数)");
    }
    const x = qi(spec.x);
    const y = qi(spec.y);
    // ウェハーマップは常にダイ単位へ集計する(「なし」は平均として扱う)
    const aggName = !spec.agg || spec.agg === "none" ? "avg" : spec.agg;
    const agg = aggName === "count" ? "COUNT(*)" : `${aggName.toUpperCase()}(${qi(spec.value)})`;
    const f = spec.facet ? `, ${qi(spec.facet)} AS f` : "";
    const f2 = spec.facet && spec.facet2 ? `, ${qi(spec.facet2)} AS f2` : "";
    const grp = [x, y, spec.facet ? qi(spec.facet) : null, spec.facet && spec.facet2 ? qi(spec.facet2) : null]
      .filter(Boolean)
      .join(", ");
    return `SELECT ${x} AS x, ${y} AS y, ${agg} AS v${f}${f2} FROM (${base}) WHERE ${x} IS NOT NULL AND ${y} IS NOT NULL GROUP BY ${grp} LIMIT 100000`;
  },
  render(ctx, w, h, spec, result, H, shared) {
    const xi = result.columns.indexOf("x");
    const yi = result.columns.indexOf("y");
    const vi = result.columns.indexOf("v");
    const valid = (rows) => rows.filter((r) => r[xi] !== null && r[yi] !== null && r[vi] !== null);
    const dies = valid(result.rows);
    if (!dies.length) {
      ctx.fillStyle = H.colors.text;
      ctx.fillText("データがありません", 16, 24);
      return;
    }
    // ファセット時は座標範囲とカラースケールを全ウェハーで共有する
    // (揃えないと同じ値が別の色になり、ウェハー同士を比較できない)
    const basis = shared && shared.allRows ? valid(shared.allRows) : dies;
    const xs = basis.map((r) => r[xi]);
    const ys = basis.map((r) => r[yi]);
    const xMin = Math.min(...xs), xMax = Math.max(...xs);
    const yMin = Math.min(...ys), yMax = Math.max(...ys);
    const [vMin, vMax] = waferRange(basis, vi);

    // ファセット時のカラースケールは格子の外側に本体が描く(renderLegend)。
    // ここで1枚だけ右余白を広げるとマップの大きさと位置が揃わなくなる
    const showScale = !shared;
    // 正方形セルでプロット領域(単独表示時は右にカラースケール分を確保)に収める
    const m = { l: 16, r: showScale ? 80 : 16, t: 12, b: 28 };
    const nx = xMax - xMin + 1;
    const ny = yMax - yMin + 1;
    const cell = Math.max(2, Math.min((w - m.l - m.r) / nx, (h - m.t - m.b) / ny));
    const gridW = cell * nx, gridH = cell * ny;
    const ox = m.l + (w - m.l - m.r - gridW) / 2;
    const oy = m.t + (h - m.t - m.b - gridH) / 2;

    // ダイ描画(Y軸は上向きが正になるよう反転)
    for (const r of dies) {
      const cx = ox + (r[xi] - xMin) * cell;
      const cy = oy + (yMax - r[yi]) * cell;
      ctx.fillStyle = waferColor(r[vi], vMin, vMax);
      ctx.fillRect(cx + 0.5, cy + 0.5, cell - 1, cell - 1);
    }
    // ウェハー外周円
    ctx.strokeStyle = H.colors.axis;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(ox + gridW / 2, oy + gridH / 2, Math.max(gridW, gridH) / 2 + cell * 0.6, 0, Math.PI * 2);
    ctx.stroke();
    ctx.lineWidth = 1;

    if (showScale) {
      // 単独表示時のカラースケール(右端の縦バー)
      drawWaferScale(ctx, w - m.r + 28, m.t + 10, h - m.t - m.b - 20, vMin, vMax, H);
    }
    ctx.fillStyle = H.colors.text;
    ctx.textAlign = "left";
    ctx.textBaseline = "alphabetic";
    ctx.fillText(`ダイ数: ${dies.length}`, m.l, h - 8);
  },
  // ファセット時のカラースケール。全ウェハー共通なので格子の外側に1つだけ描く
  // (本体が最上段の右端パネルの右隣に配置する)
  legendWidth: 76,
  renderLegend(ctx, w, h, spec, result, H) {
    const vi = result.columns.indexOf("v");
    const rows = result.rows.filter((r) => r[vi] !== null);
    if (!rows.length) return;
    const [vMin, vMax] = waferRange(rows, vi);
    // マップ本体(render の m.t / m.b)と同じ縦位置に揃える
    drawWaferScale(ctx, 12, 22, h - 60, vMin, vMax, H);
  },
});

// ---------- SPC管理図(v0.4) ----------
// 管理限界(±3σ)とネルソンルール判定は Rust 側(/api/analyze/spc)で行い、
// UIは描画だけを担当する(設計Rule 1)。SQLでは統計計算を表現できないため
// buildQuery ではなく fetch フックを使う(Plugin API ドラフト6.1の拡張)。

kohaku.registerChartType({
  type: "spc",
  label: "SPC管理図",
  form: { x: "時間/順序列", value: "測定値", agg: true, yrange: true, facet: true },
  async fetch(spec, base) {
    if (!spec.value) throw new Error("測定値の列を指定してください");
    const req = {
      source: { kind: "sql", sql: base },
      x: spec.x,
      value: spec.value,
      // 同一時点の複数測定はサブグループとして平均が既定(合計のみ明示指定)
      agg: spec.agg === "sum" ? "sum" : "avg",
    };
    // 分割指定時はグループ別実行API(装置ごとに管理限界を引き直して一括作図)。
    // 絞り込みも各グループの計算もRust側で行う(設計Rule 1)。
    // facet2 を足すと (列,行) ペアごとに実行される(2次元ファセット)
    if (spec.facet) {
      const g = { ...req, analysis: "spc", group: spec.facet };
      if (spec.facet2) g.group2 = spec.facet2;
      return api("/api/analyze/group", g);
    }
    return api("/api/analyze/spc", req);
  },
  render(ctx, w, h, spec, r, H, shared) {
    const n = r.values.length;
    const m = { l: 60, r: 52, t: 16, b: 40 };
    const pw = w - m.l - m.r;
    const ph = h - m.t - m.b;
    // ファセット時はY軸レンジを全パネルで共有する(装置ごとの水準差を
    // 見比べられるようにするため。管理限界は各パネル自身の値で描く)
    const basis = shared && shared.allResults ? shared.allResults : [r];
    const lo = Math.min(...basis.map((x) => Math.min(x.lcl, ...x.values)));
    const hi = Math.max(...basis.map((x) => Math.max(x.ucl, ...x.values)));
    const auto = H.niceTicks(lo, hi, 5);
    const range = H.applyManualRange(spec, auto, 5);
    const yTicks = range.ticks;
    const yMin = range.min;
    const yMax = range.max;
    const px = (i) => m.l + (n <= 1 ? 0 : (i / (n - 1)) * pw);
    const py = (v) => m.t + ph - ((v - yMin) / (yMax - yMin || 1)) * ph;

    // グリッドと目盛り
    ctx.strokeStyle = H.colors.grid;
    ctx.fillStyle = H.colors.text;
    ctx.textAlign = "right";
    for (const t of yTicks) {
      ctx.beginPath();
      ctx.moveTo(m.l, py(t));
      ctx.lineTo(m.l + pw, py(t));
      ctx.stroke();
      ctx.fillText(H.fmtTick(t), m.l - 6, py(t) + 4);
    }
    // 中心線と管理限界線(破線+右端にラベル)
    // 表示範囲外の管理限界線は描かない(枠外に線とラベルが出て軸ラベルと重なるため)。
    // 手動レンジで管理限界が隠れた場合は、後段の情報行で明示する。
    const hidden = [];
    const hline = (v, color, name) => {
      if (v < yMin || v > yMax) {
        hidden.push(name);
        return;
      }
      ctx.strokeStyle = color;
      ctx.setLineDash([5, 4]);
      ctx.beginPath();
      ctx.moveTo(m.l, py(v));
      ctx.lineTo(m.l + pw, py(v));
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = color;
      ctx.textAlign = "left";
      ctx.fillText(name, m.l + pw + 4, py(v) + 4);
    };
    hline(r.ucl, H.colors.danger, "UCL");
    hline(r.center, H.colors.accent2, "CL");
    hline(r.lcl, H.colors.danger, "LCL");

    // 測定値の折れ線と点(ルール違反の点は赤・大きめ)。プロット領域でクリップする
    ctx.save();
    ctx.beginPath();
    ctx.rect(m.l, m.t, pw, ph);
    ctx.clip();
    ctx.strokeStyle = H.colors.accent;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    r.values.forEach((v, i) => (i ? ctx.lineTo(px(i), py(v)) : ctx.moveTo(px(i), py(v))));
    ctx.stroke();
    ctx.lineWidth = 1;
    const bad = new Set(r.violations.map((v) => v.index));
    r.values.forEach((v, i) => {
      ctx.beginPath();
      ctx.arc(px(i), py(v), bad.has(i) ? 4 : 2, 0, Math.PI * 2);
      ctx.fillStyle = bad.has(i) ? H.colors.danger : H.colors.accent;
      ctx.fill();
    });
    ctx.restore();

    // X軸ラベル(数個に間引き)。本数はパネル幅に実際に収まる数まで減らす
    // (ファセットでパネルが狭くなるとラベル同士が重なるため)
    ctx.fillStyle = H.colors.text;
    ctx.textAlign = "center";
    const labelW = Math.max(...r.labels.map((l) => ctx.measureText(String(l)).width));
    const nt = Math.min(6, Math.max(1, Math.floor(pw / (labelW + 12))), n);
    for (let t = 0; t < nt; t++) {
      const i = Math.round((t / Math.max(1, nt - 1)) * (n - 1));
      ctx.fillText(String(r.labels[i]), px(i), m.t + ph + 16);
    }

    // 情報行(違反の内訳、なければσとn)
    const ruleNames = { 1: "3σ超え", 2: "9点連続同側", 3: "6点連続増減" };
    const byRule = {};
    for (const v of r.violations) byRule[v.rule] = (byRule[v.rule] || 0) + 1;
    const parts = Object.keys(byRule).map((k) => `ルール${k}(${ruleNames[k]}): ${byRule[k]}点`);
    ctx.textAlign = "left";
    ctx.fillStyle = r.violations.length ? H.colors.danger : H.colors.text;
    const note = hidden.length ? `(${hidden.join("・")}は表示範囲外)` : "";
    // 行数上限で打ち切られた場合は必ず添える(部分データと気づかずに管理状態を
    // 判断するのを防ぐ)
    const cut = r.truncated ? `(先頭${(r.row_limit || 0).toLocaleString()}行のみ)` : "";
    ctx.fillText(
      (r.violations.length
        ? `異常あり — ${parts.join(" / ")}`
        : `異常なし(σ=${H.fmtTick(r.sigma)}, n=${r.n_used})`) + note + cut,
      m.l,
      h - 8
    );
  },
});

// ---------- 歩留まり推移プリセット(v0.4) ----------
// 「推移(折れ線・時点平均)+SPC管理図」の定番セットをワンクリックで作成する

function openPresetModal() {
  if (!datasets.length) {
    setStatus("先にデータセットをインポートしてください", true);
    return;
  }
  const sel = $("pr-dataset");
  const cur = sel.value;
  sel.innerHTML = "";
  for (const d of datasets) {
    const op = document.createElement("option");
    op.value = d.name;
    op.textContent = d.name;
    sel.appendChild(op);
  }
  if (cur && datasets.some((d) => d.name === cur)) sel.value = cur;
  sel.onchange = fillPresetColumns;
  fillPresetColumns();
  $("pr-msg").textContent = "";
  $("preset-modal").classList.remove("hidden");
}

function fillPresetColumns() {
  const d = datasets.find((x) => x.name === $("pr-dataset").value);
  const cols = d && d.schema ? d.schema.columns : [];
  const numeric = cols.filter((c) => c.data_type === "Int64" || c.data_type === "Float64");
  const fill = (id, names, withNone) => {
    const sel = $(id);
    sel.innerHTML = "";
    if (withNone) {
      const none = document.createElement("option");
      none.value = "";
      none.textContent = "(なし)";
      sel.appendChild(none);
    }
    for (const n of names) {
      const op = document.createElement("option");
      op.value = n;
      op.textContent = n;
      sel.appendChild(op);
    }
  };
  fill("pr-x", cols.map((c) => c.name), false);
  fill("pr-y", numeric.map((c) => c.name), false);
  fill("pr-series", cols.map((c) => c.name), true);
  // 列名からの既定値の推測(外れてもユーザーが選び直すだけ)
  const guess = (id, words) => {
    const sel = $(id);
    const hit = [...sel.options].find((o) => words.some((w) => o.value.toLowerCase().includes(w)));
    if (hit) sel.value = hit.value;
  };
  guess("pr-x", ["date", "time", "day", "lot", "日付", "日時"]);
  guess("pr-y", ["yield", "歩留"]);
  guess("pr-series", ["tool", "装置", "equip"]);
}

async function createYieldPreset() {
  const ds = $("pr-dataset").value;
  const x = $("pr-x").value;
  const y = $("pr-y").value;
  const series = $("pr-series").value;
  if (!x || !y) {
    $("pr-msg").textContent = "時間/順序列と測定値の列を指定してください";
    return;
  }
  if (x === y) {
    $("pr-msg").textContent = "時間/順序列と測定値の列には別の列を指定してください";
    return;
  }
  const src = { kind: "dataset", dataset: ds };
  const t = Date.now();
  charts.push({
    id: t,
    name: `${y}の推移(${ds})`,
    chart_type: "line",
    source: src,
    x,
    y,
    value: "",
    series,
    agg: "avg",
    bins: 20,
    layout: { w: 2, h: "m" },
  });
  charts.push({
    id: t + 1,
    name: `${y}のSPC管理図(${ds})`,
    chart_type: "spc",
    source: src,
    x,
    y: "",
    value: y,
    series: "",
    agg: "avg",
    bins: 20,
    layout: { w: 2, h: "m" },
  });
  try {
    await api("/api/charts/set", { charts });
    renderChartList();
    $("preset-modal").classList.add("hidden");
    setStatus(`「${y}の推移」と「${y}のSPC管理図」を作成しました`);
    switchTab("dashboard");
  } catch (e) {
    $("pr-msg").textContent = e.message;
  }
}

// ---------- ロットトレース(v0.4) ----------

/** ID列候補 = 全データセットの列名の和集合。既定は "lot" を含む列 */
function renderLotColSelect() {
  const sel = $("lot-col");
  if (!sel) return;
  const cur = sel.value;
  const names = [];
  for (const d of datasets) {
    if (!d.schema) continue;
    for (const c of d.schema.columns) {
      if (!names.includes(c.name)) names.push(c.name);
    }
  }
  sel.innerHTML = "";
  for (const n of names) {
    const op = document.createElement("option");
    op.value = n;
    op.textContent = n;
    sel.appendChild(op);
  }
  if (cur && names.includes(cur)) {
    sel.value = cur;
  } else {
    const lot = names.find((n) => n.toLowerCase().includes("lot"));
    if (lot) sel.value = lot;
  }
}

async function runLotTrace() {
  const out = $("lot-out");
  const value = $("lot-id").value.trim();
  if (!value) {
    out.innerHTML = '<div class="hint error">検索するIDを入力してください</div>';
    return;
  }
  out.innerHTML = '<div class="hint">検索中...</div>';
  try {
    const r = await api("/api/analyze/lottrace", {
      column: $("lot-col").value,
      value,
      partial: $("lot-partial").checked,
    });
    if (!r.results.length) {
      out.innerHTML = `<div class="hint">「${esc(r.value)}」は見つかりませんでした(列 ${esc(r.column)} を持つ ${r.searched_datasets} データセットを検索)</div>`;
      return;
    }
    const total = r.results.reduce((a, x) => a + x.rows.length, 0);
    out.innerHTML = `<div class="est-box">「${esc(r.value)}」の記録: ${r.results.length} データセット / 計 ${total.toLocaleString()} 行(${r.searched_datasets} データセットを検索)</div>`;
    for (const res of r.results) {
      const head = document.createElement("h3");
      head.textContent = `${res.dataset}(${res.rows.length.toLocaleString()}行${res.truncated ? "、先頭のみ表示" : ""})`;
      out.appendChild(head);
      const wrap = document.createElement("div");
      wrap.className = "table-wrap";
      out.appendChild(wrap);
      renderTable(wrap, res.columns, res.rows, 500);
    }
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

// ---------- 装置差分析(v0.4) ----------
// 検定・多重比較はすべて Rust 側(/api/analyze/tooldiff)。UIは表示のみ

async function runToolDiff() {
  const out = $("tool-out");
  const req = { source: anGetSource(), group: $("tool-g").value, value: $("tool-v").value };
  out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/tooldiff", req);
    const t = r.test;
    const eff = t.effect
      ? `, ${esc(t.effect.name)}=${fmtNum(t.effect.value, 3)}(${esc(t.effect.magnitude)})`
      : "";
    let html = truncWarnHtml(r);
    html += `<div class="${t.significant ? "rec-box" : "est-box"}"><b>${
      t.significant ? "⚠ グループ間に有意差あり" : "グループ間に有意差なし"
    }</b>(${esc(t.name)}, ${esc(t.statistic_name)}=${fmtNum(t.statistic, 3)}, p=${fmtP(t.p_value)}${eff})</div>`;
    // 群別統計(平均の昇順)。有意差ありのとき最下位を赤で強調
    const gs = [...r.groups].sort((a, b) => a.mean - b.mean);
    html +=
      '<div class="table-wrap"><table class="grid"><thead><tr><th>グループ</th><th>n</th><th>平均</th><th>SD</th><th>最小</th><th>最大</th></tr></thead><tbody>';
    gs.forEach((g, i) => {
      const mark = i === 0 && t.significant ? ' style="color:var(--danger)"' : "";
      html += `<tr${mark}><td>${esc(g.label)}</td><td class="num">${g.n.toLocaleString()}</td><td class="num">${fmtNum(g.mean, 3)}</td><td class="num">${fmtNum(g.sd, 3)}</td><td class="num">${fmtNum(g.min, 3)}</td><td class="num">${fmtNum(g.max, 3)}</td></tr>`;
    });
    html += "</tbody></table></div>";
    if (Array.isArray(r.pairs)) {
      const sig = r.pairs.filter((p) => p.significant);
      html += sig.length
        ? `<div class="hint">有意差のあるペア(Holm補正後 p&lt;${r.alpha}): ` +
          sig
            .map((p) => `${esc(p.a)} vs ${esc(p.b)}(平均差 ${fmtNum(p.mean_diff, 3)}, p=${fmtP(p.p_adjusted)})`)
            .join(" / ") +
          "</div>"
        : '<div class="hint">Holm補正後に有意なペアはありません</div>';
    }
    if (r.dropped_small.length) {
      html += `<div class="hint">検定から除外(3点未満): ${esc(r.dropped_small.join(", "))}</div>`;
    }
    out.innerHTML = html;
    drawStripPlot($("tool-canvas"), r.groups);
  } catch (e) {
    $("tool-canvas").classList.add("hidden");
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** ストリップ図: グループごとの測定値の分布をジッター付きの点で示す */
function drawStripPlot(canvas, groups) {
  registerRedraw(canvas, () => drawStripPlot(canvas, groups));
  canvas.classList.remove("hidden");
  const { ctx, w, h } = setupCanvas(canvas);
  const C = CHART_COLORS;
  const m = { l: 60, r: 16, t: 16, b: 46 };
  const pw = w - m.l - m.r;
  const ph = h - m.t - m.b;
  const all = groups.flatMap((g) => g.points);
  const ticks = niceTicks(Math.min(...all), Math.max(...all), 5);
  const yMin = ticks[0];
  const yMax = ticks[ticks.length - 1];
  const py = (v) => m.t + ph - ((v - yMin) / (yMax - yMin || 1)) * ph;
  const gx = (gi) => m.l + ((gi + 0.5) / groups.length) * pw;

  ctx.strokeStyle = C.grid;
  ctx.fillStyle = C.text;
  ctx.textAlign = "right";
  for (const t of ticks) {
    ctx.beginPath();
    ctx.moveTo(m.l, py(t));
    ctx.lineTo(m.l + pw, py(t));
    ctx.stroke();
    ctx.fillText(fmtTick(t), m.l - 6, py(t) + 4);
  }
  // 全体平均(表示点の単純平均)の破線
  const total = all.reduce((a, b) => a + b, 0) / all.length;
  ctx.strokeStyle = C.accent2;
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(m.l, py(total));
  ctx.lineTo(m.l + pw, py(total));
  ctx.stroke();
  ctx.setLineDash([]);

  const jitterW = Math.min(40, (pw / groups.length) * 0.35);
  ctx.save();
  ctx.beginPath();
  ctx.rect(m.l, m.t, pw, ph);
  ctx.clip();
  groups.forEach((g, gi) => {
    // 点(決定的ジッターで再描画しても同じ見た目にする)
    ctx.globalAlpha = 0.55;
    ctx.fillStyle = SERIES_COLORS[gi % SERIES_COLORS.length];
    g.points.forEach((v, i) => {
      const j = ((((i + 1) * 2654435761) >>> 16) % 1000) / 1000 - 0.5;
      ctx.beginPath();
      ctx.arc(gx(gi) + j * 2 * jitterW, py(v), 2, 0, Math.PI * 2);
      ctx.fill();
    });
    ctx.globalAlpha = 1;
    // 平均の横棒
    ctx.strokeStyle = C.text;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(gx(gi) - jitterW, py(g.mean));
    ctx.lineTo(gx(gi) + jitterW, py(g.mean));
    ctx.stroke();
    ctx.lineWidth = 1;
  });
  ctx.restore();
  // グループ名(クリップ外に描く)
  ctx.fillStyle = C.text;
  ctx.textAlign = "center";
  groups.forEach((g, gi) => ctx.fillText(g.label, gx(gi), m.t + ph + 16, pw / groups.length - 8));
  ctx.textAlign = "left";
  ctx.fillStyle = C.accent2;
  ctx.fillText("--- 全体平均", m.l + 4, m.t - 4);
}

function anGetSource() {
  return $("an-source-kind").value === "dataset"
    ? { kind: "dataset", dataset: $("an-dataset").value }
    : { kind: "sql", sql: stripSemi($("an-sql").value) };
}

function renderAnDatasetSelect() {
  const sel = $("an-dataset");
  const cur = sel.value;
  sel.innerHTML = "";
  for (const d of datasets) {
    const op = document.createElement("option");
    op.value = d.name;
    op.textContent = d.name;
    sel.appendChild(op);
  }
  if (cur && datasets.some((d) => d.name === cur)) sel.value = cur;
}

// ソースの列一覧(数値判定つき)を取得して各選択UIへ反映
async function anLoadColumns() {
  anColumns = [];
  $("an-cols-msg").textContent = "";
  try {
    const kind = $("an-source-kind").value;
    if (kind === "dataset") {
      const d = datasets.find((x) => x.name === $("an-dataset").value);
      if (d && d.schema) {
        anColumns = d.schema.columns.map((c) => ({
          name: c.name,
          numeric: c.data_type === "Int64" || c.data_type === "Float64",
        }));
      }
    } else {
      const sql = stripSemi($("an-sql").value);
      if (sql) {
        const r = await api("/api/query", { sql: `SELECT * FROM (${sql}) LIMIT 100`, limit: 100 });
        anColumns = r.columns.map((name, i) => {
          let numeric = false;
          for (const row of r.rows) {
            if (row[i] === null) continue;
            if (typeof row[i] === "number") { numeric = true; } else { numeric = false; break; }
          }
          return { name, numeric };
        });
      }
    }
  } catch (e) {
    $("an-cols-msg").textContent = e.message;
  }
  const numCols = anColumns.filter((c) => c.numeric);
  // 回帰: Y select + X checklist
  const regY = $("reg-y");
  regY.innerHTML = "";
  for (const c of numCols) {
    const op = document.createElement("option");
    op.value = c.name;
    op.textContent = c.name;
    regY.appendChild(op);
  }
  const mkChecklist = (container, prefix) => {
    container.innerHTML = "";
    for (const c of numCols) {
      const lb = document.createElement("label");
      lb.innerHTML = `<input type="checkbox" value="${esc(c.name)}"> ${esc(c.name)}`;
      container.appendChild(lb);
    }
  };
  mkChecklist($("reg-x"));
  mkChecklist($("clu-x"));
  // 時系列分解: 時間列は全列から、値の列は数値列から選ぶ
  const fillCols = (id, cols, optional) => {
    const sel = $(id);
    const cur = sel.value;
    sel.innerHTML = "";
    if (optional) {
      const none = document.createElement("option");
      none.value = "";
      none.textContent = "(分割しない)";
      sel.appendChild(none);
    }
    for (const c of cols) {
      const op = document.createElement("option");
      op.value = c.name;
      op.textContent = c.name;
      sel.appendChild(op);
    }
    if (cur && cols.some((c) => c.name === cur)) sel.value = cur;
  };
  fillCols("ts-x", anColumns);
  fillCols("ts-y", numCols);
  fillCols("tool-g", anColumns);
  fillCols("tool-v", numCols);
  // グループ分割は任意指定(空欄=分割しない)
  fillCols("ts-group", anColumns, true);
  fillCols("reg-group", anColumns, true);
  fillCols("an-profile-group", anColumns, true);
  fillCols("clu-group", anColumns, true);
  renderTestSelects();
  if (!numCols.length) $("an-cols-msg").textContent = "数値列がありません(ソースを選択してください)";
}

// ---------- 自動検定 (Test Advisor) ----------

function fillSelect(id, items) {
  const sel = $(id);
  const cur = sel.value;
  sel.innerHTML = "";
  for (const name of items) {
    const op = document.createElement("option");
    op.value = name;
    op.textContent = name;
    sel.appendChild(op);
  }
  const kept = items.includes(cur);
  if (kept) sel.value = cur;
  return kept; // false = 既定値にリセットされた
}

/** p値の表示: 極小値は「<0.0001」 */
function fmtP(p) {
  if (p === null || p === undefined || !isFinite(p)) return "—";
  if (p > 0 && p < 0.0001) return "<0.0001";
  return fmtNum(p, 4);
}

function renderTestSelects() {
  const num = anColumns.filter((c) => c.numeric).map((c) => c.name);
  const cat = anColumns.filter((c) => !c.numeric).map((c) => c.name);
  const all = anColumns.map((c) => c.name);
  fillSelect("tst-target", num);
  fillSelect("tst-x", num);
  fillSelect("tst-y", num);
  // グループ/カテゴリ選択は、ユーザー選択が失われた時のみカテゴリ列を既定にする
  if (!fillSelect("tst-group", all) && cat.length) $("tst-group").value = cat[0];
  if (!fillSelect("tst-rowcol", all) && cat.length) $("tst-rowcol").value = cat[0];
  if (!fillSelect("tst-colcol", all) && cat.length) $("tst-colcol").value = cat[cat.length > 1 ? 1 : 0];
  if (!fillSelect("tst-prop-col", all) && cat.length) $("tst-prop-col").value = cat[0];
  // Y列は既定でX列と別の列に
  if (num.length > 1 && $("tst-y").value === $("tst-x").value) $("tst-y").value = num[1];
}

function updateTestModeVisibility() {
  const m = $("tst-mode").value;
  $("tst-target-row").classList.toggle("hidden", m !== "groups" && m !== "one_sample");
  $("tst-group-row").classList.toggle("hidden", m !== "groups");
  $("tst-indep-row").classList.toggle("hidden", m !== "groups");
  $("tst-x-row").classList.toggle("hidden", m !== "two_numeric");
  $("tst-y-row").classList.toggle("hidden", m !== "two_numeric");
  $("tst-paired-row").classList.toggle("hidden", m !== "two_numeric");
  $("tst-row-row").classList.toggle("hidden", m !== "categorical");
  $("tst-col-row").classList.toggle("hidden", m !== "categorical");
  $("tst-mu-row").classList.toggle("hidden", m !== "one_sample");
  $("tst-propcol-row").classList.toggle("hidden", m !== "proportion");
  $("tst-success-row").classList.toggle("hidden", m !== "proportion");
  $("tst-p0-row").classList.toggle("hidden", m !== "proportion");
}

function tstBody() {
  const mode = $("tst-mode").value;
  const b = {
    source: anGetSource(),
    mode,
    alpha: parseFloat($("tst-alpha").value) || 0.05,
    correction: $("tst-correction").value,
  };
  if (mode === "groups") {
    b.target = $("tst-target").value;
    b.group = $("tst-group").value;
  } else if (mode === "two_numeric") {
    b.x = $("tst-x").value;
    b.y = $("tst-y").value;
    b.paired = $("tst-paired").value === "true";
  } else if (mode === "one_sample") {
    b.target = $("tst-target").value;
    b.mu0 = parseFloat($("tst-mu").value) || 0;
  } else if (mode === "proportion") {
    b.column = $("tst-prop-col").value;
    b.success = $("tst-success").value;
    b.p0 = parseFloat($("tst-p0").value) || 0.5;
  } else {
    b.row = $("tst-rowcol").value;
    b.col = $("tst-colcol").value;
  }
  return b;
}

function assumptionTable(items) {
  if (!items || !items.length) return "";
  let h = '<h4>前提条件チェック</h4><div class="table-wrap"><table class="grid"><thead><tr><th>項目</th><th>統計量</th><th>p値</th><th>判定</th><th>コメント</th></tr></thead><tbody>';
  for (const a of items) {
    const mark = a.passed ? '<span style="color:var(--ok)">OK</span>' : '<span style="color:var(--warn)">要注意</span>';
    h += `<tr><td>${esc(a.name)}</td><td class="num">${fmtNum(a.statistic, 3)}</td><td class="num">${fmtP(a.p_value)}</td><td>${mark}</td><td>${esc(a.note)}</td></tr>`;
  }
  return h + "</tbody></table></div>";
}

function groupTable(items) {
  if (!items || !items.length) return "";
  let h = '<div class="table-wrap"><table class="grid"><thead><tr><th>群</th><th>n</th><th>平均</th><th>標準偏差</th></tr></thead><tbody>';
  for (const g of items) {
    h += `<tr><td>${esc(g.label)}</td><td class="num">${g.n}</td><td class="num">${fmtNum(g.mean, 4)}</td><td class="num">${fmtNum(g.sd, 4)}</td></tr>`;
  }
  return h + "</tbody></table></div>";
}

/** 比率モード用: カテゴリ/件数/比率 の表(group_summariesを流用) */
function propTable(items) {
  if (!items || !items.length) return "";
  let h = '<div class="table-wrap"><table class="grid"><thead><tr><th>カテゴリ</th><th>件数</th><th>比率</th></tr></thead><tbody>';
  for (const g of items) {
    h += `<tr><td>${esc(g.label)}</td><td class="num">${g.n}</td><td class="num">${fmtNum(g.mean, 4)}</td></tr>`;
  }
  return h + "</tbody></table></div>";
}

/** 独立性の申告に応じた注意文(仕様: 独立性はデータだけでは判定できない) */
function independenceWarning() {
  if ($("tst-mode").value !== "groups") return "";
  const v = $("tst-indep").value;
  if (v === "before_after") {
    return '<div class="hint" style="color:var(--warn)">⚠ 同じ対象のbefore/after比較には、分析タイプ「2つの数値列」→「対応あり」を使用してください。独立群として検定すると誤った結果になります。</div>';
  }
  if (v === "repeated") {
    return '<div class="hint" style="color:var(--warn)">⚠ 同じロット・装置・個体の繰り返し測定は独立ではない可能性があります。検定の前提(独立性)が崩れるため、結果は参考程度に留めてください。</div>';
  }
  if (v === "unknown") {
    return '<div class="hint" style="color:var(--warn)">⚠ サンプルの独立性が不明です。同一対象・同一ロットからの繰り返し測定が含まれる場合、p値は当てになりません。</div>';
  }
  return "";
}

async function tstAdvise() {
  const out = $("tst-advice");
  out.innerHTML = '<div class="hint">診断中...</div>';
  $("tst-run-row").classList.add("hidden");
  $("btn-tst-md").classList.add("hidden");
  $("tst-result").innerHTML = "";
  try {
    const mode = $("tst-mode").value;
    const r = await api("/api/analyze/advise", tstBody());
    let html = independenceWarning();
    html += `<h4>目的: ${esc(r.intent)}</h4>`;
    html += `<div class="rec-box"><div>第一候補: <b>${esc(r.primary_label)}</b></div>`;
    if (r.alternatives.length) html += `<div class="hint">代替候補: ${r.alternatives.map((a) => esc(a.label)).join(" / ")}</div>`;
    html += "</div>";
    if (r.reasons.length) html += "<h4>理由</h4><ul>" + r.reasons.map((x) => `<li>${esc(x)}</li>`).join("") + "</ul>";
    if (r.warnings.length) html += "<h4>注意</h4><ul>" + r.warnings.map((x) => `<li>⚠ ${esc(x)}</li>`).join("") + "</ul>";
    html += assumptionTable(r.assumptions);
    if (mode === "proportion") {
      html += propTable(r.group_summaries);
      // 成功カテゴリの選択肢を件数の多い順で用意
      fillSelect("tst-success", r.group_summaries.map((g) => g.label));
    } else {
      html += groupTable(r.group_summaries);
    }
    out.innerHTML = html;
    // 実行する検定の候補を用意(第一候補を既定に)
    const sel = $("tst-choice");
    sel.innerHTML = "";
    for (const o of r.available) {
      const op = document.createElement("option");
      op.value = o.id;
      op.textContent = o.label + (o.id === r.primary ? "(推奨)" : "");
      sel.appendChild(op);
    }
    sel.value = r.primary;
    $("tst-run-row").classList.remove("hidden");
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

async function tstRun() {
  const out = $("tst-result");
  out.innerHTML = '<div class="hint">検定中...</div>';
  try {
    const body = tstBody();
    body.test = $("tst-choice").value;
    const r = await api("/api/analyze/test", body);
    window.__lastTest = { body, resp: r };
    const t = r.result;
    const metrics = [[t.statistic_name, fmtNum(t.statistic, 4)]];
    if (t.df !== null && t.df !== undefined) metrics.push([t.df2 !== null && t.df2 !== undefined ? "自由度 df1" : "自由度 df", fmtNum(t.df, t.df2 != null ? 0 : 2)]);
    if (t.df2 !== null && t.df2 !== undefined) metrics.push(["自由度 df2", fmtNum(t.df2, 2)]);
    metrics.push(["p値", fmtP(t.p_value)]);
    if (t.effect) metrics.push([t.effect.name + `(${t.effect.magnitude})`, fmtNum(t.effect.value, 3)]);
    if (r.power !== null && r.power !== undefined) metrics.push(["検出力(参考値)", fmtNum(r.power, 3)]);
    metrics.push(["n", t.n]);
    let html = `<h4>${esc(t.test)}</h4>`;
    html += metricHtml(metrics);
    html += `<div class="hint">帰無仮説: ${esc(t.null_hypothesis)} / 対立仮説(両側): 帰無仮説は成り立たない</div>`;
    if (r.power !== null && r.power !== undefined) html += '<div class="hint">検出力は観測効果量に基づく事後推定(正規近似)の参考値です。検定の解釈はp値・効果量・信頼区間を優先してください。</div>';
    if (t.estimate !== null && t.estimate !== undefined) {
      let est = `${esc(t.estimate_label || "推定値")}: <b>${fmtNum(t.estimate, 4)}</b>`;
      if (t.ci) est += ` &nbsp; ${Math.round(t.ci.level * 100)}% 信頼区間 [${fmtNum(t.ci.low, 4)}, ${fmtNum(t.ci.high, 4)}]`;
      html += `<div class="est-box">${est}</div>`;
    }
    const sigColor = t.p_value < (parseFloat($("tst-alpha").value) || 0.05) ? "var(--ok)" : "var(--muted)";
    html += `<div class="interp" style="border-left:3px solid ${sigColor}">${esc(t.interpretation)}</div>`;
    if (t.warnings && t.warnings.length) html += "<ul>" + t.warnings.map((x) => `<li>⚠ ${esc(x)}</li>`).join("") + "</ul>";
    html += groupTable(t.groups);
    // 事後検定(多重比較)
    if (r.posthoc && r.posthoc.pairs) {
      html += `<h4>事後のペアワイズ比較 (${esc(r.posthoc.method)}, 補正: ${esc(r.correction)})</h4>`;
      html += '<div class="table-wrap"><table class="grid"><thead><tr><th>群A</th><th>群B</th><th>統計量</th><th>効果量</th><th>p (未補正)</th><th>p (補正後)</th><th>有意</th></tr></thead><tbody>';
      for (const p of r.posthoc.pairs) {
        const mark = p.significant ? '<span style="color:var(--ok)">✔</span>' : "";
        html += `<tr><td>${esc(p.a)}</td><td>${esc(p.b)}</td><td class="num">${fmtNum(p.statistic, 3)}</td><td class="num">${fmtNum(p.effect, 3)}</td><td class="num">${fmtP(p.p)}</td><td class="num">${fmtP(p.p_adjusted)}</td><td>${mark}</td></tr>`;
      }
      html += "</tbody></table></div>";
    }
    html += `<div class="hint" style="margin-top:8px">${esc(r.note)}</div>`;
    out.innerHTML = html;
    $("btn-tst-md").classList.remove("hidden");
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** 最後の検定結果をMarkdownレポートとして組み立てる */
function tstMarkdown() {
  const lt = window.__lastTest;
  if (!lt) return "";
  const t = lt.resp.result;
  const p = (v) => (v > 0 && v < 0.0001 ? "<0.0001" : v === null || v === undefined ? "—" : Number(v.toFixed(4)));
  const lines = [`## ${t.test}`, ""];
  lines.push(`- 帰無仮説: ${t.null_hypothesis}`);
  lines.push(`- ${t.statistic_name} = ${Number(t.statistic.toFixed(4))}`);
  if (t.df != null) lines.push(`- 自由度: ${Number(t.df.toFixed(2))}${t.df2 != null ? " / " + Number(t.df2.toFixed(2)) : ""}`);
  lines.push(`- p値: ${p(t.p_value)}`);
  if (t.estimate != null) {
    let est = `- ${t.estimate_label || "推定値"}: ${Number(t.estimate.toFixed(4))}`;
    if (t.ci) est += `(${Math.round(t.ci.level * 100)}% CI [${Number(t.ci.low.toFixed(4))}, ${Number(t.ci.high.toFixed(4))}])`;
    lines.push(est);
  }
  if (t.effect) lines.push(`- 効果量 ${t.effect.name}: ${Number(t.effect.value.toFixed(3))}(${t.effect.magnitude})`);
  if (lt.resp.power != null) lines.push(`- 検出力(参考値): ${Number(lt.resp.power.toFixed(3))}`);
  lines.push(`- n = ${t.n}`);
  if (t.groups && t.groups.length) {
    lines.push("", "| 群 | n | 平均 | SD |", "|---|---|---|---|");
    for (const g of t.groups) lines.push(`| ${g.label} | ${g.n} | ${Number(g.mean.toFixed(4))} | ${isFinite(g.sd) ? Number(g.sd.toFixed(4)) : "—"} |`);
  }
  if (t.warnings && t.warnings.length) {
    lines.push("", "### 注意");
    for (const w of t.warnings) lines.push(`- ${w}`);
  }
  const ph = lt.resp.posthoc;
  if (ph && ph.pairs) {
    lines.push("", `### 事後のペアワイズ比較(${ph.method} / 補正: ${lt.resp.correction})`);
    lines.push("| 群A | 群B | 統計量 | 効果量 | p | p(補正後) | 有意 |", "|---|---|---|---|---|---|---|");
    for (const x of ph.pairs) lines.push(`| ${x.a} | ${x.b} | ${x.statistic} | ${x.effect ?? "—"} | ${p(x.p)} | ${p(x.p_adjusted)} | ${x.significant ? "✔" : ""} |`);
  }
  lines.push("", `> 解釈: ${t.interpretation}`, `> ${lt.resp.note}`, "", `_Kohaku Test Advisor / ${new Date().toISOString().slice(0, 10)}_`);
  return lines.join("\n");
}

async function tstCopyMarkdown() {
  const md = tstMarkdown();
  if (!md) return;
  try {
    await navigator.clipboard.writeText(md);
    setStatus("Markdownをコピーしました");
  } catch (_) {
    // クリップボードAPI不可の環境ではダウンロードにフォールバック
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([md], { type: "text/markdown" }));
    a.download = "test_result.md";
    a.click();
    URL.revokeObjectURL(a.href);
  }
}

function checkedValues(container) {
  return [...container.querySelectorAll("input:checked")].map((x) => x.value);
}

function metricHtml(items) {
  return '<div class="metrics">' + items.map(([l, v]) => `<div class="metric"><div class="mv">${esc(String(v))}</div><div class="ml">${esc(l)}</div></div>`).join("") + "</div>";
}

function fmtNum(v, digits) {
  if (v === null || v === undefined || (typeof v === "number" && !isFinite(v))) return "—";
  if (typeof v !== "number") return esc(String(v));
  return Number(v.toFixed(digits === undefined ? 4 : digits)).toLocaleString(undefined, { maximumFractionDigits: digits === undefined ? 4 : digits });
}

// --- プロファイル ---

async function runProfile() {
  const out = $("an-profile-out");
  const group = $("an-profile-group").value;
  const req = { source: anGetSource() };
  if (group) {
    await runGrouped("profile", group, req, out, (box, r) => {
      box.insertAdjacentHTML("beforeend", profileSummaryHtml(r));
    });
    return;
  }
  out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/profile", req);
    out.innerHTML = profileSummaryHtml(r);
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** プロファイル結果のHTML(単独実行・グループ別実行の共通処理) */
/** 行数上限で打ち切られたときの警告。部分データと気づかずに結論を出すのを防ぐため、
 *  分析結果の先頭に必ず出す(サーバーが truncated / row_limit を返す)。 */
function truncWarnHtml(r) {
  if (!r || !r.truncated) return "";
  const lim = (r.row_limit || 0).toLocaleString();
  return `<div class="hint warn-text">データが上限を超えたため、先頭${lim}行だけで分析しています（結果は全体を表していません）</div>`;
}

function profileSummaryHtml(r) {
  let html = `<div class="hint">${r.n_rows.toLocaleString()}行${r.truncated ? "(上限で打ち切り)" : ""}</div>`;
  html += truncWarnHtml(r);
  // 列統計
  html += "<h4>列統計</h4>";
  html += '<div class="table-wrap"><table class="grid"><thead><tr><th>列</th><th>型</th><th>件数</th><th>NULL</th><th>個別値</th><th>平均</th><th>標準偏差</th><th>最小</th><th>25%</th><th>中央値</th><th>75%</th><th>最大</th></tr></thead><tbody>';
  for (const c of r.columns) {
    const s = c.stats || {};
    html += `<tr><td>${esc(c.name)}</td><td>${c.kind === "numeric" ? "数値" : "文字列"}</td>
      <td class="num">${c.count.toLocaleString()}</td><td class="num">${c.nulls.toLocaleString()}</td>
      <td class="num">${c.distinct.toLocaleString()}${c.distinct_capped ? "+" : ""}</td>
      <td class="num">${fmtNum(s.mean)}</td><td class="num">${fmtNum(s.std)}</td>
      <td class="num">${fmtNum(s.min)}</td><td class="num">${fmtNum(s.q25)}</td>
      <td class="num">${fmtNum(s.median)}</td><td class="num">${fmtNum(s.q75)}</td>
      <td class="num">${fmtNum(s.max)}</td></tr>`;
  }
  html += "</tbody></table></div>";
  // 相関行列
  const corr = r.correlation;
  if (corr.columns.length >= 2) {
    html += "<h4>相関行列(ピアソン)</h4>";
    html += '<div class="table-wrap"><table class="grid corr"><thead><tr><th></th>';
    for (const c of corr.columns) html += `<th>${esc(c)}</th>`;
    html += "</tr></thead><tbody>";
    corr.matrix.forEach((row, i) => {
      html += `<tr><th>${esc(corr.columns[i])}</th>`;
      row.forEach((v, j) => {
        if (i === j) { html += '<td class="diag">1</td>'; return; }
        if (v === null) { html += "<td>—</td>"; return; }
        const alpha = Math.min(0.85, Math.abs(v));
        // 正の相関=データ既定色 / 負の相関=danger。どちらもテーマ変数から取る
        const bg = rgbaVar(v >= 0 ? "--chart-primary" : "--danger", alpha);
        html += `<td style="background:${bg}">${fmtNum(v, 2)}</td>`;
      });
      html += "</tr>";
    });
    html += "</tbody></table></div>";
  }
  // 強相関ペア
  if (r.top_pairs.length) {
    html += "<h4>相関の強いペア</h4><ul>";
    for (const p of r.top_pairs.slice(0, 5)) {
      html += `<li>${esc(p.a)} × ${esc(p.b)} : r = ${fmtNum(p.r, 3)}</li>`;
    }
    html += "</ul>";
  }
  return html;
}

// --- グループ別実行(v0.7) ---
// グループ値ごとの絞り込みと分析の実行は Rust 側(/api/analyze/group)。
// 返る結果は単独実行時と同じ形なので、UIは同じ描画関数を値の数だけ繰り返す。

/** グループ分割ありで分析を実行し、値ごとのパネルを out に並べる。
 *  1パネルの描画は renderInto(box, result) に任せる(単独実行と共通の関数)。 */
async function runGrouped(analysis, group, req, out, renderInto) {
  out.innerHTML = '<div class="hint">グループ別に実行中...</div>';
  let r;
  try {
    r = await api("/api/analyze/group", { ...req, analysis, group });
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
    return;
  }
  out.innerHTML = "";
  if (r.truncated) {
    const note = document.createElement("div");
    note.className = "hint";
    note.textContent = `${r.group}: ${r.total}件中 先頭${r.shown}件を表示`;
    out.appendChild(note);
  }
  for (const g of r.groups) {
    const box = document.createElement("div");
    box.className = "an-group";
    const h = document.createElement("h4");
    h.textContent = `${r.group} = ${g.value}`;
    box.appendChild(h);
    out.appendChild(box); // 先にDOMへ入れる(Canvasのサイズ確定に必要)
    if (g.error) {
      // 1グループの失敗で他を消さない(サーバー側も止めずに返している)
      const e = document.createElement("div");
      e.className = "hint error";
      e.textContent = g.error;
      box.appendChild(e);
    } else {
      renderInto(box, g.result);
    }
  }
}

/** グループのパネルに結果表示用のCanvasを作って返す */
function anGroupCanvas(box, cls) {
  const canvas = document.createElement("canvas");
  canvas.className = `an-canvas ${cls || ""}`;
  box.appendChild(canvas);
  return canvas;
}

// --- 回帰 ---

async function runRegression() {
  const out = $("reg-out");
  const target = $("reg-y").value;
  const features = checkedValues($("reg-x")).filter((f) => f !== target);
  const group = $("reg-group").value;
  const req = { source: anGetSource(), target, features };
  $("reg-canvas").classList.add("hidden");
  if (group) {
    await runGrouped("regression", group, req, out, (box, r) => {
      box.insertAdjacentHTML("beforeend", regSummaryHtml(r));
      regDraw(anGroupCanvas(box), r);
    });
    return;
  }
  out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/regression", req);
    out.innerHTML = regSummaryHtml(r);
    const canvas = $("reg-canvas");
    canvas.classList.remove("hidden");
    regDraw(canvas, r);
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** 回帰結果の要約HTML(単独実行・グループ別実行の共通処理) */
function regSummaryHtml(r) {
  let html = truncWarnHtml(r);
  html += metricHtml([
    ["決定係数 R²", fmtNum(r.r2, 4)],
    ["自由度調整済み R²", fmtNum(r.adj_r2, 4)],
    ["RMSE", fmtNum(r.rmse, 4)],
    ["サンプル数", r.n.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped}` + ")" : "")],
  ]);
  html += '<div class="table-wrap"><table class="grid"><thead><tr><th>項</th><th>係数</th><th>標準誤差</th><th>t値</th></tr></thead><tbody>';
  r.names.forEach((n, i) => {
    html += `<tr><td>${esc(n)}</td><td class="num">${fmtNum(r.coef[i], 6)}</td>
      <td class="num">${fmtNum(r.stderr[i], 6)}</td><td class="num">${fmtNum(r.tvalues[i], 2)}</td></tr>`;
  });
  html += "</tbody></table></div>";
  // 回帰式
  let eq = `${esc(r.target)} = ${fmtNum(r.coef[0], 4)}`;
  r.names.slice(1).forEach((n, i) => {
    const c = r.coef[i + 1];
    eq += ` ${c >= 0 ? "+" : "−"} ${fmtNum(Math.abs(c), 4)} × ${esc(n)}`;
  });
  html += `<div class="hint">回帰式: ${eq}</div>`;
  return html;
}

/** 回帰の散布図(単独実行・グループ別実行の共通処理) */
function regDraw(canvas, r) {
  canvas.classList.remove("hidden");
  const pts = r.points.map((p) => ({ x: p[0], y: p[1] }));
  if (r.single_feature) {
    drawAnScatter(canvas, pts, {
      xlab: r.feature, ylab: r.target,
      line: { slope: r.coef[1], intercept: r.coef[0] },
    });
  } else {
    drawAnScatter(canvas, pts, { xlab: "実測値", ylab: "予測値", diag: true });
  }
}

// --- クラスタリング ---

async function runCluster(saveAs) {
  const out = $("clu-out");
  const features = checkedValues($("clu-x"));
  const k = parseInt($("clu-k").value, 10) || 3;
  const req = { source: anGetSource(), features, k };
  if (saveAs) req.save_as = saveAs;
  const group = $("clu-group").value;
  if (group && !saveAs) {
    // グループ別実行では軸の選択・結果の保存は使えない(パネルごとに
    // 別の結果になるため)。散布図は先頭2特徴量で描く
    $("clu-axes-row").classList.add("hidden");
    $("clu-canvas").classList.add("hidden");
    await runGrouped("cluster", group, req, out, (box, r) => {
      box.insertAdjacentHTML("beforeend", clusterSummaryHtml(r));
      if (r.features.length > 1) {
        drawClusterScatterInto(anGroupCanvas(box), r, 0, 1);
      }
    });
    return;
  }
  if (!saveAs) out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/cluster", req);
    lastCluster = r;
    lastClusterReq = { source: req.source, features, k };
    let html = clusterSummaryHtml(r);
    if (r.saved) {
      html += `<div class="hint">データセット「${esc(r.saved.name)}」として保存しました(${r.saved.rows.toLocaleString()}行、cluster列付き)</div>`;
      await refreshState();
    }
    out.innerHTML = html;

    // 軸選択を用意して散布図描画
    const axRow = $("clu-axes-row");
    axRow.classList.remove("hidden");
    for (const id of ["clu-ax", "clu-ay"]) {
      const sel = $(id);
      const cur = sel.value;
      sel.innerHTML = "";
      r.features.forEach((f, i) => {
        const op = document.createElement("option");
        op.value = i;
        op.textContent = f;
        sel.appendChild(op);
      });
      if (cur !== "" && cur < r.features.length) sel.value = cur;
    }
    if (r.features.length > 1 && $("clu-ax").value === $("clu-ay").value) $("clu-ay").value = 1;
    drawClusterScatter();
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** エルボー法でクラスタ数kを提案し、曲線を描画してユーザーが確認できるようにする */
async function suggestClusterK() {
  const out = $("clu-out");
  const features = checkedValues($("clu-x"));
  out.innerHTML = '<div class="hint">エルボー法で計算中(k=1〜10でクラスタリングを試行)...</div>';
  try {
    const r = await api("/api/analyze/elbow", { source: anGetSource(), features, k_max: 10 });
    $("clu-k").value = r.suggested_k;
    out.innerHTML =
      metricHtml([
        ["提案されたk", r.suggested_k],
        ["使用行数", r.n_used.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped})` : "")],
      ]) +
      '<div class="hint">WCSS(クラスタ内二乗和)の減少が緩やかになる「肘」の位置を提案しました。' +
      "曲線を確認し、必要ならkを調整してから実行してください。</div>";
    $("clu-axes-row").classList.add("hidden"); // 軸選択は散布図用なので隠す
    drawElbowChart($("clu-canvas"), r);
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** エルボー曲線(k vs WCSS)。提案kの位置を強調表示する */
function drawElbowChart(canvas, r) {
  registerRedraw(canvas, () => drawElbowChart(canvas, r));
  canvas.classList.remove("hidden");
  const { ctx, w, h } = setupCanvas(canvas);
  const C = CHART_COLORS;
  const m = { l: 60, r: 16, t: 14, b: 44 };
  const pw = w - m.l - m.r;
  const ph = h - m.t - m.b;
  const kMin = r.ks[0];
  const kMax = r.ks[r.ks.length - 1];
  const yTicks = niceTicks(0, Math.max(...r.inertias), 5);
  const yMax = yTicks[yTicks.length - 1] || 1;
  const px = (k) => m.l + ((k - kMin) / Math.max(1, kMax - kMin)) * pw;
  const py = (v) => m.t + ph - (v / yMax) * ph;

  // 軸・目盛り
  ctx.strokeStyle = C.grid;
  ctx.fillStyle = C.text;
  ctx.textAlign = "right";
  for (const t of yTicks) {
    ctx.beginPath();
    ctx.moveTo(m.l, py(t));
    ctx.lineTo(m.l + pw, py(t));
    ctx.stroke();
    ctx.fillText(fmtTick(t), m.l - 6, py(t) + 4);
  }
  ctx.textAlign = "center";
  for (const k of r.ks) ctx.fillText(String(k), px(k), m.t + ph + 16);
  ctx.fillText("クラスタ数 k", m.l + pw / 2, m.t + ph + 34);
  ctx.save();
  ctx.translate(14, m.t + ph / 2);
  ctx.rotate(-Math.PI / 2);
  ctx.fillText("WCSS(クラスタ内二乗和)", 0, 0);
  ctx.restore();

  // 提案kの縦線(破線)
  ctx.strokeStyle = C.accent2;
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(px(r.suggested_k), m.t);
  ctx.lineTo(px(r.suggested_k), m.t + ph);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.fillStyle = C.accent2;
  ctx.fillText(`提案 k=${r.suggested_k}`, px(r.suggested_k), m.t + 10);

  // 曲線と点(プロット領域でクリップ)
  ctx.save();
  ctx.beginPath();
  ctx.rect(m.l, m.t, pw, ph);
  ctx.clip();
  ctx.strokeStyle = C.accent;
  ctx.lineWidth = 2;
  ctx.beginPath();
  r.ks.forEach((k, i) => {
    if (i === 0) ctx.moveTo(px(k), py(r.inertias[i]));
    else ctx.lineTo(px(k), py(r.inertias[i]));
  });
  ctx.stroke();
  ctx.lineWidth = 1;
  r.ks.forEach((k, i) => {
    ctx.beginPath();
    ctx.arc(px(k), py(r.inertias[i]), k === r.suggested_k ? 5 : 3, 0, Math.PI * 2);
    ctx.fillStyle = k === r.suggested_k ? C.accent2 : C.accent;
    ctx.fill();
  });
  ctx.restore();
  ctx.textAlign = "left";
}

/** クラスタリング結果の要約HTML(単独実行・グループ別実行の共通処理) */
function clusterSummaryHtml(r) {
  let html = truncWarnHtml(r);
  html += metricHtml([
    ["クラスタ数", r.k],
    ["使用行数", r.n_used.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped})` : "")],
    ["慣性(小さいほど凝集)", fmtNum(r.inertia, 1)],
    ["反復回数", r.iterations],
  ]);
  html += '<div class="table-wrap"><table class="grid"><thead><tr><th>クラスタ</th><th>件数</th>';
  for (const f of r.features) html += `<th>${esc(f)} (中心)</th>`;
  html += "</tr></thead><tbody>";
  r.sizes.forEach((sz, c) => {
    html += `<tr><td><span style="color:${CLUSTER_COLORS[c % CLUSTER_COLORS.length]}">■</span> ${c}</td><td class="num">${sz.toLocaleString()}</td>`;
    r.centroids[c].forEach((v) => (html += `<td class="num">${fmtNum(v, 3)}</td>`));
    html += "</tr>";
  });
  html += "</tbody></table></div>";
  return html;
}

function drawClusterScatter() {
  if (!lastCluster) return;
  drawClusterScatterInto(
    $("clu-canvas"),
    lastCluster,
    parseInt($("clu-ax").value, 10) || 0,
    parseInt($("clu-ay").value, 10) || 0,
  );
}

/** クラスタ散布図を指定のCanvasに描く(グループ別実行ではパネルごとに呼ぶ) */
function drawClusterScatterInto(canvas, r, xi, yi) {
  canvas.classList.remove("hidden");
  const nf = r.features.length;
  const pts = r.points.map((p) => ({ x: p[xi], y: p[yi], c: p[nf] }));
  drawAnScatter(canvas, pts, {
    xlab: r.features[xi],
    ylab: r.features[yi],
    colored: true,
  });
}

// ---------- 時系列分解 ----------

async function runTimeseries() {
  const out = $("ts-out");
  const req = {
    source: anGetSource(),
    x: $("ts-x").value,
    y: $("ts-y").value,
    period: parseInt($("ts-period").value, 10) || 7,
    model: $("ts-model").value,
    agg: $("ts-agg").value,
  };
  const group = $("ts-group").value;
  if (group) {
    $("ts-canvas").classList.add("hidden");
    await runGrouped("timeseries", group, req, out, (box, r) => {
      box.insertAdjacentHTML("beforeend", tsSummaryHtml(r));
      drawDecomposition(anGroupCanvas(box, "ts-canvas"), r);
    });
    return;
  }
  out.innerHTML = '<div class="hint">分解中...</div>';
  try {
    const r = await api("/api/analyze/timeseries", req);
    out.innerHTML = tsSummaryHtml(r);
    drawDecomposition($("ts-canvas"), r);
  } catch (e) {
    $("ts-canvas").classList.add("hidden");
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

/** 時系列分解の要約HTML(単独実行・グループ別実行の共通処理) */
function tsSummaryHtml(r) {
  const judge = (v) => (v >= 0.6 ? "強い" : v >= 0.3 ? "中程度" : "弱い");
  let html = truncWarnHtml(r);
  html += metricHtml([
    ["トレンド強度", `${fmtNum(r.trend_strength, 2)}(${judge(r.trend_strength)})`],
    ["季節性強度", `${fmtNum(r.seasonal_strength, 2)}(${judge(r.seasonal_strength)})`],
    ["使用時点数", r.n_used.toLocaleString() + (r.dropped ? `(除外 ${r.dropped}行)` : "")],
    ["周期", r.period],
  ]);
  html += '<div class="table-wrap"><table class="grid"><thead><tr><th>位相(周期内の位置)</th>';
  r.seasonal_pattern.forEach((_, i) => (html += `<th>${i + 1}</th>`));
  html += "</tr></thead><tbody><tr><td>季節成分</td>";
  r.seasonal_pattern.forEach((v) => (html += `<td class="num">${fmtNum(v, 3)}</td>`));
  html += "</tr></tbody></table></div>";
  html +=
    '<div class="hint">強度は0〜1(1に近いほど成分が支配的)。両端のトレンド・残差は移動平均の性質上、計算対象外です。' +
    (r.sampled ? "表示は間引いています(分解は全時点で実行済み)。" : "") +
    "</div>";
  return html;
}

/** 分解結果を3段(観測+トレンド / 季節成分 / 残差)で描画する */
function drawDecomposition(canvas, r) {
  registerRedraw(canvas, () => drawDecomposition(canvas, r));
  canvas.classList.remove("hidden");
  const { ctx, w, h } = setupCanvas(canvas);
  const C = CHART_COLORS;
  const m = { l: 64, r: 12, t: 20, b: 34, gapY: 30 };
  const n = r.observed.length;
  const px = (i) => m.l + (n <= 1 ? 0 : (i / (n - 1)) * (w - m.l - m.r));
  const panelH = (h - m.t - m.b - m.gapY * 2) / 3;

  const drawPanel = (pi, title, seriesList, zeroLine) => {
    const top = m.t + pi * (panelH + m.gapY);
    let vals = seriesList.flatMap((s) => s.data.filter((v) => v !== null));
    if (!vals.length) vals = [0, 1];
    const ticks = niceTicks(Math.min(...vals), Math.max(...vals), 3);
    const yMin = ticks[0];
    const yMax = ticks[ticks.length - 1];
    const py = (v) => top + panelH - ((v - yMin) / (yMax - yMin || 1)) * panelH;
    ctx.strokeStyle = C.grid;
    ctx.fillStyle = C.text;
    ctx.textAlign = "right";
    for (const t of ticks) {
      ctx.beginPath();
      ctx.moveTo(m.l, py(t));
      ctx.lineTo(w - m.r, py(t));
      ctx.stroke();
      ctx.fillText(fmtTick(t), m.l - 6, py(t) + 4);
    }
    ctx.textAlign = "left";
    ctx.fillStyle = C.text;
    ctx.fillText(title, m.l, top - 7);
    if (zeroLine !== undefined && zeroLine >= yMin && zeroLine <= yMax) {
      ctx.strokeStyle = C.text;
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(m.l, py(zeroLine));
      ctx.lineTo(w - m.r, py(zeroLine));
      ctx.stroke();
      ctx.setLineDash([]);
    }
    // パネルからはみ出さないようにクリップ
    ctx.save();
    ctx.beginPath();
    ctx.rect(m.l, top, w - m.l - m.r, panelH);
    ctx.clip();
    for (const s of seriesList) {
      if (s.points) {
        ctx.fillStyle = s.color;
        s.data.forEach((v, i) => {
          if (v === null) return;
          ctx.beginPath();
          ctx.arc(px(i), py(v), 1.5, 0, Math.PI * 2);
          ctx.fill();
        });
      } else {
        ctx.strokeStyle = s.color;
        ctx.lineWidth = s.width || 1.5;
        ctx.beginPath();
        let pen = false; // nullで線を切る(トレンド両端のNaN対策)
        s.data.forEach((v, i) => {
          if (v === null) {
            pen = false;
            return;
          }
          if (pen) ctx.lineTo(px(i), py(v));
          else ctx.moveTo(px(i), py(v));
          pen = true;
        });
        ctx.stroke();
        ctx.lineWidth = 1;
      }
    }
    ctx.restore();
  };

  drawPanel(0, "観測値", [
    { data: r.observed, color: CHART_COLORS.muted },
    { data: r.trend, color: C.accent, width: 2 },
  ]);
  ctx.fillStyle = C.accent;
  ctx.fillText("── トレンド", m.l + 70, m.t - 7);
  drawPanel(1, "季節成分", [{ data: r.seasonal, color: C.accent2 }]);
  drawPanel(2, "残差", [{ data: r.residual, color: CHART_COLORS.warn, points: true }],
    r.model === "multiplicative" ? 1 : 0);

  // X軸ラベル(最下段の下に数個)
  ctx.fillStyle = C.text;
  ctx.textAlign = "center";
  const nticks = Math.min(6, n);
  for (let t = 0; t < nticks; t++) {
    const i = Math.round((t / Math.max(1, nticks - 1)) * (n - 1));
    ctx.fillText(String(r.labels[i]), px(i), h - m.b + 16);
  }
  ctx.textAlign = "left";
}

// 汎用散布図(回帰線・対角線・クラスタ色分け対応)
function drawAnScatter(canvas, pts, opts) {
  registerRedraw(canvas, () => drawAnScatter(canvas, pts, opts));
  const { ctx, w, h } = setupCanvas(canvas);
  const C = CHART_COLORS;
  const m = { l: 60, r: 16, t: 14, b: 44 };
  const pw = w - m.l - m.r, ph = h - m.t - m.b;
  const valid = pts.filter((p) => isFinite(p.x) && isFinite(p.y));
  if (!valid.length) {
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
    return;
  }
  let xMin = Math.min(...valid.map((p) => p.x)), xMax = Math.max(...valid.map((p) => p.x));
  let yMin = Math.min(...valid.map((p) => p.y)), yMax = Math.max(...valid.map((p) => p.y));
  if (opts.diag) {
    xMin = yMin = Math.min(xMin, yMin);
    xMax = yMax = Math.max(xMax, yMax);
  }
  const xTicks = niceTicks(xMin, xMax, 6);
  const yTicks = niceTicks(yMin, yMax, 5);
  xMin = Math.min(xMin, xTicks[0]); xMax = Math.max(xMax, xTicks[xTicks.length - 1]);
  yMin = Math.min(yMin, yTicks[0]); yMax = Math.max(yMax, yTicks[yTicks.length - 1]);
  const px = (x) => m.l + ((x - xMin) / ((xMax - xMin) || 1)) * pw;
  const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;

  ctx.strokeStyle = C.grid;
  ctx.fillStyle = C.text;
  ctx.lineWidth = 1;
  for (const t of yTicks) {
    ctx.beginPath(); ctx.moveTo(m.l, py(t)); ctx.lineTo(w - m.r, py(t)); ctx.stroke();
    ctx.textAlign = "right"; ctx.textBaseline = "middle";
    ctx.fillText(fmtTick(t), m.l - 6, py(t));
  }
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  for (const t of xTicks) {
    if (px(t) >= m.l - 1 && px(t) <= w - m.r + 1) ctx.fillText(fmtTick(t), px(t), m.t + ph + 6);
  }
  const rad = Math.max(1.5, 4 - Math.log10(valid.length + 1));
  // データの描画はプロット領域でクリップする。回帰直線はX範囲の両端で
  // Y範囲を超えることがあり、そのままだと軸ラベルの上に描かれてしまう
  ctx.save();
  ctx.beginPath();
  ctx.rect(m.l, m.t, pw, ph);
  ctx.clip();
  for (const p of valid) {
    ctx.fillStyle = opts.colored
      ? CLUSTER_COLORS[(p.c || 0) % CLUSTER_COLORS.length] + "b0"
      : C.accent + "8c";
    ctx.beginPath();
    ctx.arc(px(p.x), py(p.y), rad, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.strokeStyle = C.accent2;
  ctx.lineWidth = 1.8;
  if (opts.line) {
    ctx.beginPath();
    ctx.moveTo(px(xMin), py(opts.line.intercept + opts.line.slope * xMin));
    ctx.lineTo(px(xMax), py(opts.line.intercept + opts.line.slope * xMax));
    ctx.stroke();
  }
  if (opts.diag) {
    ctx.setLineDash([5, 4]);
    ctx.beginPath();
    ctx.moveTo(px(xMin), py(xMin));
    ctx.lineTo(px(xMax), py(xMax));
    ctx.stroke();
    ctx.setLineDash([]);
  }
  ctx.restore();
  ctx.lineWidth = 1;
  ctx.fillStyle = C.text;
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  ctx.fillText(opts.xlab || "", m.l + pw / 2, h - 14);
  ctx.save();
  ctx.translate(12, m.t + ph / 2);
  ctx.rotate(-Math.PI / 2);
  ctx.textBaseline = "middle";
  ctx.fillText(opts.ylab || "", 0, 0);
  ctx.restore();
}

// ---------- ダッシュボード ----------

let dashRenderSeq = 0; // 並行描画ガード(タブ切替+更新ボタンの二重実行対策)
const dashCache = new Map(); // chartId → クエリ結果。レイアウト操作時の再クエリを避ける

// ---------- グローバルフィルタ(ウィジェット間連動) ----------

let dashFilters = []; // [{col, values:[...]}] セッション内のみ保持
const dashSourceCols = new Map(); // "sql:"+chartId → SQLソースの列一覧キャッシュ

/** 全データセット横断の列名一覧(重複なし) */
function allDatasetColumns() {
  const seen = new Set();
  for (const d of datasets) {
    if (!d.schema) continue;
    for (const c of d.schema.columns) seen.add(c.name);
  }
  return [...seen].sort();
}

function renderFilterColSelect() {
  const sel = $("dash-filter-col");
  const cur = sel.value;
  sel.innerHTML = '<option value="">(列を選択)</option>';
  for (const c of allDatasetColumns()) {
    const op = document.createElement("option");
    op.value = c;
    op.textContent = c;
    sel.appendChild(op);
  }
  if ([...sel.options].some((o) => o.value === cur)) sel.value = cur;
}

/** 選択列の候補値(列を持つ全データセットのUNION、先頭100件)をチェックリストに出す */
async function loadFilterValues() {
  const col = $("dash-filter-col").value;
  const box = $("dash-filter-vals");
  const btn = $("btn-dash-filter-apply");
  box.innerHTML = "";
  box.classList.toggle("hidden", !col);
  btn.classList.toggle("hidden", !col);
  if (!col) return;
  const dss = datasets.filter((d) => d.schema && d.schema.columns.some((c) => c.name === col));
  if (!dss.length) return;
  const union = dss.map((d) => `SELECT ${qi(col)} AS v FROM ${qi(d.name)}`).join(" UNION ");
  try {
    const r = await api("/api/query", {
      sql: `SELECT DISTINCT v FROM (${union}) WHERE v IS NOT NULL ORDER BY v LIMIT 100`,
      limit: 100,
    });
    const active = dashFilters.find((f) => f.col === col);
    for (const row of r.rows) {
      const v = row[0];
      const lb = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = !!(active && active.values.some((x) => x === v));
      cb.__val = v; // 数値/文字列の型を保持(SQLリテラルの書き方が変わる)
      lb.appendChild(cb);
      lb.appendChild(document.createTextNode(" " + String(v)));
      box.appendChild(lb);
    }
    if (r.rows.length >= 100) {
      const hint = document.createElement("span");
      hint.className = "hint";
      hint.textContent = "(先頭100件のみ表示)";
      box.appendChild(hint);
    }
  } catch (e) {
    box.innerHTML = `<span class="hint error">${esc(e.message)}</span>`;
  }
}

function renderFilterChips() {
  const wrap = $("dash-filter-chips");
  wrap.innerHTML = "";
  for (const f of dashFilters) {
    const chip = document.createElement("span");
    chip.className = "dash-chip";
    const label =
      f.values.slice(0, 3).map(String).join(", ") + (f.values.length > 3 ? ` +${f.values.length - 3}` : "");
    chip.innerHTML = `<b>${esc(f.col)}</b>: ${esc(label)} <button title="このフィルタを外す">×</button>`;
    chip.querySelector("button").onclick = () => {
      dashFilters = dashFilters.filter((x) => x !== f);
      renderFilterChips();
      renderDashboard(true);
    };
    wrap.appendChild(chip);
  }
}

function applyDashFilter() {
  const col = $("dash-filter-col").value;
  if (!col) return;
  const values = [...$("dash-filter-vals").querySelectorAll("input:checked")].map((cb) => cb.__val);
  dashFilters = dashFilters.filter((f) => f.col !== col);
  if (values.length) dashFilters.push({ col, values });
  renderFilterChips();
  // ピッカーを畳む
  $("dash-filter-col").value = "";
  $("dash-filter-vals").classList.add("hidden");
  $("btn-dash-filter-apply").classList.add("hidden");
  renderDashboard(true);
}

/** チャートのソースが持つ列一覧(フィルタ適用可否の判定用) */
async function chartSourceCols(spec) {
  if (spec.source.kind === "dataset") {
    const d = datasets.find((x) => x.name === spec.source.dataset);
    return d && d.schema ? d.schema.columns.map((c) => c.name) : [];
  }
  const key = "sql:" + spec.id;
  if (dashSourceCols.has(key)) return dashSourceCols.get(key);
  try {
    const r = await api("/api/query", { sql: `SELECT * FROM (${chartBaseSql(spec)}) LIMIT 1`, limit: 1 });
    dashSourceCols.set(key, r.columns);
    return r.columns;
  } catch (e) {
    return [];
  }
}

/** チャートのレイアウト設定(未設定は 1列幅 × 中高さ) */
function chartLayout(spec) {
  const l = spec.layout || {};
  return { w: l.w === 2 ? 2 : 1, h: l.h === "s" || l.h === "l" ? l.h : "m" };
}

function applyCardLayout(card, spec) {
  const l = chartLayout(spec);
  card.classList.toggle("wide", l.w === 2);
  card.classList.toggle("h-s", l.h === "s");
  card.classList.toggle("h-l", l.h === "l");
}

async function saveChartsQuiet() {
  try {
    await api("/api/charts/set", { charts });
  } catch (e) {
    setStatus(e.message, true);
  }
}

/** カードのレイアウト操作(幅・高さ・並び順)。設定はチャート定義として保存される */
async function dashAction(act, spec, card, canvas, tdiv) {
  const l = chartLayout(spec);
  if (act === "width") {
    spec.layout = { ...l, w: l.w === 2 ? 1 : 2 };
  } else if (act === "height") {
    spec.layout = { ...l, h: { s: "m", m: "l", l: "s" }[l.h] };
  } else {
    // 並び替え: charts配列とDOMを同時に入れ替える(再クエリなし)
    const i = charts.findIndex((c) => c.id === spec.id);
    const j = act === "left" ? i - 1 : i + 1;
    if (i < 0 || j < 0 || j >= charts.length) return;
    [charts[i], charts[j]] = [charts[j], charts[i]];
    if (act === "left") card.parentElement.insertBefore(card, card.previousElementSibling);
    else if (card.nextElementSibling) card.parentElement.insertBefore(card.nextElementSibling, card);
    await saveChartsQuiet();
    return;
  }
  applyCardLayout(card, spec);
  // サイズ変更後はキャッシュ結果で再描画(Canvasは要素サイズに自動追従しないため)
  const r = dashCache.get(spec.id);
  if (r) drawChartInto(canvas, tdiv, spec, r);
  await saveChartsQuiet();
}

async function renderDashboard(force) {
  const seq = ++dashRenderSeq;
  if (force === true) dashCache.clear();
  const grid = $("dash-grid");
  grid.innerHTML = "";
  if (!charts.length) {
    grid.innerHTML = '<div class="hint">保存済みチャートがありません。「チャート」タブで作成・保存してください。</div>';
    return;
  }
  renderFilterColSelect();
  for (const spec of charts) {
    if (seq !== dashRenderSeq) return; // 新しい描画に取って代わられたら中断
    // このチャートに適用できるフィルタと、列が無く適用外のフィルタを仕分ける
    const cols = await chartSourceCols(spec);
    if (seq !== dashRenderSeq) return;
    const applicable = dashFilters.filter((f) => cols.includes(f.col));
    const na = dashFilters.filter((f) => !cols.includes(f.col));
    const naHtml = na.length
      ? `<span class="dash-na">フィルタ対象外: ${esc(na.map((f) => f.col).join(", "))}</span>`
      : "";
    const card = document.createElement("div");
    card.className = "dash-card";
    applyCardLayout(card, spec);
    const head = document.createElement("div");
    head.className = "dash-head";
    head.innerHTML = `<h4>${esc(spec.name)}${naHtml}</h4>
      <button class="dash-btn" data-act="left" title="左へ移動">${ICON.left}</button>
      <button class="dash-btn" data-act="right" title="右へ移動">${ICON.right}</button>
      <button class="dash-btn" data-act="width" title="幅を切替(1列 / 2列)">${ICON.width}</button>
      <button class="dash-btn" data-act="height" title="高さを切替(小 / 中 / 大)">${ICON.height}</button>`;
    const canvas = document.createElement("canvas");
    const tdiv = document.createElement("div");
    tdiv.className = "table-wrap hidden";
    card.appendChild(head);
    card.appendChild(canvas);
    card.appendChild(tdiv);
    grid.appendChild(card);
    head.querySelectorAll(".dash-btn").forEach((b) => {
      b.onclick = () => dashAction(b.dataset.act, spec, card, canvas, tdiv);
    });
    try {
      let r = dashCache.get(spec.id);
      if (!r) {
        r = await chartData(spec, applicable);
        dashCache.set(spec.id, r);
      }
      if (seq !== dashRenderSeq) return;
      drawChartInto(canvas, tdiv, spec, r);
    } catch (err) {
      card.innerHTML += `<div class="hint error">${esc(err.message)}</div>`;
    }
  }
}

// ---------- エクスポート(PNG / HTMLレポート) ----------

/** style.css のCSS変数を取得(エクスポート画像の配色をアプリと一致させる) */
function cssVar(name, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

/** テーマ変数の色(#rrggbb)を半透明のrgba()にする。ヒートマップの塗り用 */
function rgbaVar(name, alpha) {
  const hex = cssVar(name, "#4f8ef7").replace("#", "");
  const n = parseInt(hex.length === 3 ? hex.replace(/./g, (c) => c + c) : hex.slice(0, 6), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

function tsStamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}`;
}

function safeFilename(s) {
  return (s || "export").replace(/[\\/:*?"<>|]/g, "_").slice(0, 60);
}

function downloadDataUrl(url, filename) {
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
}

/** Canvasは透明背景(CSS任せ)のため、背景色を合成した不透明PNGを作る */
function opaquePngDataUrl(canvas) {
  const out = document.createElement("canvas");
  out.width = canvas.width;
  out.height = canvas.height;
  const ctx = out.getContext("2d");
  ctx.fillStyle = cssVar("--bg2", "#1f232b");
  ctx.fillRect(0, 0, out.width, out.height);
  ctx.drawImage(canvas, 0, 0);
  return out.toDataURL("image/png");
}

/** チャートタブ: プレビュー中のチャートをタイトル付きPNGで保存 */
function exportChartPng() {
  const canvas = $("chart-canvas");
  const spec = chartSpecFromForm();
  if (spec.chart_type === "table") {
    setStatus("テーブルはPNGに対応していません(ダッシュボードのHTMLレポートを使ってください)", true);
    return;
  }
  if (canvas.classList.contains("hidden") || !canvas.width) {
    setStatus("先にプレビューを実行してください", true);
    return;
  }
  const dpr = window.devicePixelRatio || 1;
  const titleH = Math.round(30 * dpr);
  const out = document.createElement("canvas");
  out.width = canvas.width;
  out.height = canvas.height + titleH;
  const ctx = out.getContext("2d");
  ctx.fillStyle = cssVar("--bg2", "#1f232b");
  ctx.fillRect(0, 0, out.width, out.height);
  ctx.fillStyle = cssVar("--text", "#dde2ea");
  ctx.font = `bold ${Math.round(14 * dpr)}px 'Yu Gothic UI', sans-serif`;
  ctx.fillText(spec.name, Math.round(12 * dpr), Math.round(20 * dpr));
  ctx.drawImage(canvas, 0, titleH);
  downloadDataUrl(out.toDataURL("image/png"), `${safeFilename(spec.name)}.png`);
  setStatus(`PNGを保存しました: ${spec.name}`);
}

/** ダッシュボードPNG用: テーブル型チャートを簡易表として描き込む */
function drawTableInto(ctx, spec, box) {
  const r = spec ? dashCache.get(spec.id) : null;
  ctx.save();
  ctx.beginPath();
  ctx.rect(box.x, box.y, box.w, box.h);
  ctx.clip();
  ctx.font = "10px 'Yu Gothic UI', sans-serif";
  if (!r) {
    ctx.fillStyle = cssVar("--muted", "#8a93a3");
    ctx.fillText("(データ未取得)", box.x, box.y + 12);
    ctx.restore();
    return;
  }
  const cols = r.columns.slice(0, 6);
  const cw = box.w / cols.length;
  const lh = 16;
  ctx.fillStyle = cssVar("--muted", "#8a93a3");
  cols.forEach((cname, ci) => ctx.fillText(String(cname), box.x + ci * cw, box.y + 10, cw - 6));
  ctx.fillStyle = cssVar("--text", "#dde2ea");
  const maxRows = Math.max(0, Math.floor((box.h - lh) / lh));
  r.rows.slice(0, maxRows).forEach((row, ri) => {
    cols.forEach((_, ci) => {
      const v = row[ci];
      ctx.fillText(v === null ? "" : String(v), box.x + ci * cw, box.y + 10 + (ri + 1) * lh, cw - 6);
    });
  });
  ctx.restore();
}

function roundRectPath(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

/** ダッシュボード全体を1枚のPNGに合成して保存(見た目のレイアウトを維持) */
function exportDashPng() {
  const grid = $("dash-grid");
  const cards = [...grid.querySelectorAll(".dash-card")];
  if (!cards.length) {
    setStatus("ダッシュボードにチャートがありません", true);
    return;
  }
  const gr = grid.getBoundingClientRect();
  const scale = 2; // 印刷にも耐える解像度で書き出す
  const height = Math.max(...cards.map((c) => c.getBoundingClientRect().bottom)) - gr.top + 8;
  const out = document.createElement("canvas");
  out.width = Math.round(gr.width * scale);
  out.height = Math.round(height * scale);
  const ctx = out.getContext("2d");
  ctx.scale(scale, scale);
  ctx.fillStyle = cssVar("--bg", "#17191f");
  ctx.fillRect(0, 0, gr.width, height);
  cards.forEach((card, i) => {
    const r = card.getBoundingClientRect();
    const x = r.left - gr.left;
    const y = r.top - gr.top;
    ctx.fillStyle = cssVar("--bg2", "#1f232b");
    ctx.strokeStyle = cssVar("--border", "#363c48");
    roundRectPath(ctx, x, y, r.width, r.height, 6);
    ctx.fill();
    ctx.stroke();
    const h4 = card.querySelector("h4");
    ctx.fillStyle = cssVar("--text", "#dde2ea");
    ctx.font = "bold 13px 'Yu Gothic UI', sans-serif";
    ctx.fillText(h4 && h4.firstChild ? h4.firstChild.textContent : "", x + 10, y + 20, r.width - 20);
    const cv = card.querySelector("canvas");
    if (cv && !cv.classList.contains("hidden") && cv.width) {
      const cr = cv.getBoundingClientRect();
      ctx.drawImage(cv, cr.left - gr.left, cr.top - gr.top, cr.width, cr.height);
    } else {
      drawTableInto(ctx, charts[i], { x: x + 10, y: y + 30, w: r.width - 20, h: r.height - 40 });
    }
  });
  const name = $("project-name").textContent || "dashboard";
  downloadDataUrl(out.toDataURL("image/png"), `${safeFilename(name)}_${tsStamp()}.png`);
  setStatus("ダッシュボードをPNGで保存しました");
}

/** HTMLレポート用: クエリ結果を静的な<table>にする */
function htmlTable(result, cap) {
  if (!result) return '<p class="muted">(データ未取得)</p>';
  const head = result.columns.map((c) => `<th>${esc(c)}</th>`).join("");
  const body = result.rows
    .slice(0, cap)
    .map((row) => `<tr>${row.map((v) => `<td>${v === null ? "" : esc(String(v))}</td>`).join("")}</tr>`)
    .join("");
  const note = result.rows.length > cap ? `<p class="muted">先頭${cap}行のみ表示(全${result.rows.length}行)</p>` : "";
  return `<div class="twrap"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>${note}`;
}

/** ダッシュボードを自己完結HTML(画像埋め込み・オフラインで開ける)として保存 */
function exportDashHtml() {
  const grid = $("dash-grid");
  const cards = [...grid.querySelectorAll(".dash-card")];
  if (!cards.length) {
    setStatus("ダッシュボードにチャートがありません", true);
    return;
  }
  const name = $("project-name").textContent || "ダッシュボード";
  const filters = dashFilters.length
    ? `<p class="muted">適用フィルタ: ${esc(dashFilters.map((f) => `${f.col} ∈ {${f.values.join(", ")}}`).join(" / "))}</p>`
    : "";
  const cardsHtml = cards
    .map((card, i) => {
      const spec = charts[i];
      const layout = spec ? chartLayout(spec) : { w: 1 };
      const title = spec ? esc(spec.name) : "";
      const cv = card.querySelector("canvas");
      const body =
        cv && !cv.classList.contains("hidden") && cv.width
          ? `<img src="${opaquePngDataUrl(cv)}" alt="${title}">`
          : htmlTable(spec ? dashCache.get(spec.id) : null, 200);
      return `<div class="card${layout.w === 2 ? " wide" : ""}"><h2>${title}</h2>${body}</div>`;
    })
    .join("\n");
  const html = `<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<title>${esc(name)}</title>
<style>
  body { margin: 0; padding: 20px; background: ${cssVar("--bg", "#17191f")}; color: ${cssVar("--text", "#dde2ea")};
         font-family: 'Yu Gothic UI', 'Hiragino Sans', sans-serif; }
  h1 { font-size: 20px; margin: 0 0 4px; }
  .muted { color: ${cssVar("--muted", "#8a93a3")}; font-size: 12px; margin: 2px 0; }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(420px, 1fr)); gap: 12px; margin-top: 16px; }
  .card { background: ${cssVar("--bg2", "#1f232b")}; border: 1px solid ${cssVar("--border", "#363c48")};
          border-radius: 6px; padding: 12px; }
  .card.wide { grid-column: 1 / -1; }
  .card h2 { font-size: 14px; margin: 0 0 8px; }
  .card img { width: 100%; height: auto; border-radius: 4px; }
  .twrap { max-height: 420px; overflow: auto; }
  table { border-collapse: collapse; width: 100%; font-size: 12px; }
  th, td { border: 1px solid ${cssVar("--border", "#363c48")}; padding: 3px 8px; text-align: left; }
  th { background: ${cssVar("--bg3", "#282d37")}; position: sticky; top: 0; }
</style>
</head>
<body>
<h1>${esc(name)}</h1>
<p class="muted">Kohaku Studio エクスポート ${new Date().toLocaleString("ja-JP")}</p>
${filters}
<div class="grid">
${cardsHtml}
</div>
</body>
</html>
`;
  const blob = new Blob([html], { type: "text/html" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `${safeFilename(name)}_${tsStamp()}.html`;
  a.click();
  URL.revokeObjectURL(a.href);
  setStatus("ダッシュボードをHTMLレポートで保存しました");
}

// ---------- プロジェクト ----------

let projectMode = "save";

function openProjectModal(mode) {
  projectMode = mode;
  $("prj-title").textContent = mode === "save" ? "プロジェクトを保存" : "プロジェクトを開く";
  $("prj-msg").textContent = "";
  $("prj-path").value = localStorage.getItem("kohaku.lastProject") || "";
  $("project-modal").classList.remove("hidden");
}

async function projectOk() {
  const path = $("prj-path").value.trim();
  if (!path) return;
  $("prj-msg").textContent = "処理中...";
  try {
    if (projectMode === "save") {
      const r = await api("/api/project/save", { path });
      localStorage.setItem("kohaku.lastProject", r.path);
      $("project-modal").classList.add("hidden");
      setStatus(`保存しました: ${r.path}`);
      await refreshState();
    } else {
      const r = await api("/api/project/load", { path });
      localStorage.setItem("kohaku.lastProject", path);
      $("project-modal").classList.add("hidden");
      currentDataset = null;
      await refreshState();
      if (r.errors && r.errors.length) {
        setStatus("一部読み込み失敗: " + r.errors.join(" / "), true);
      } else {
        setStatus(`プロジェクトを開きました: ${r.project_name}`);
      }
    }
  } catch (err) {
    $("prj-msg").textContent = err.message;
  }
}

// ---------- 初期化 ----------

function init() {
  initTheme(); // 配色を確定してから描画を始める
  $("btn-theme").onclick = () =>
    applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
  document.querySelectorAll(".tab").forEach((t) => (t.onclick = () => switchTab(t.dataset.tab)));
  document.querySelectorAll(".close").forEach((b) => (b.onclick = () => $(b.dataset.close).classList.add("hidden")));

  $("btn-import").onclick = openImport;
  $("imp-go").onclick = () => browse($("imp-path").value);
  $("imp-path").addEventListener("keydown", (e) => { if (e.key === "Enter") browse($("imp-path").value); });
  $("imp-tab-file").onclick = () => impMode("file");
  $("imp-tab-db").onclick = () => impMode("db");
  $("imp-db-connect").onclick = connectDb;
  $("imp-db-url").addEventListener("keydown", (e) => { if (e.key === "Enter") connectDb(); });
  $("imp-object").onchange = () => {
    if (imp.connector === "excel" && imp.objects.length > 1) {
      const stem = imp.path.split("\\").pop().replace(/\.[^.]+$/, "");
      $("imp-name").value = sanitizeName(stem + "_" + $("imp-object").value);
    } else if (imp.connector === "postgres" || imp.connector === "mysql") {
      $("imp-name").value = sanitizeName($("imp-object").value);
    }
    refreshImportPreview();
  };
  $("imp-header").onchange = refreshImportPreview;
  $("imp-delim").onchange = refreshImportPreview;
  $("imp-import").onclick = doImport;

  $("btn-run-sql").onclick = runSql;
  $("sql-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); runSql(); }
  });
  // ハイライトは入力・スクロールのたびに背面へ描き直す
  $("sql-input").addEventListener("input", syncSqlHighlight);
  $("sql-input").addEventListener("scroll", syncSqlHighlight);
  $("sql-history").onchange = () => {
    if ($("sql-history").value) {
      $("sql-input").value = $("sql-history").value;
      syncSqlHighlight(); // 履歴からの流し込みは input が発火しない
    }
  };
  $("btn-sql-csv").onclick = exportCsv;
  $("btn-sql-chart").onclick = () => {
    const sql = $("sql-input").value.trim();
    if (!sql) return;
    editingChartId = null;
    $("ch-source-kind").value = "sql";
    $("ch-sql").value = stripSemi(sql);
    updateChartFormVisibility();
    loadChartColumns();
    switchTab("charts");
  };

  // 登録チャートタイプをセレクトへ追加(組み込み5種はHTML側で定義済み)
  for (const def of CHART_REGISTRY.values()) {
    const op = document.createElement("option");
    op.value = def.type;
    op.textContent = def.label;
    $("ch-type").appendChild(op);
  }
  $("ch-type").onchange = updateChartFormVisibility;
  $("ch-facet").onchange = syncFacet2Enabled;
  $("ch-source-kind").onchange = () => { updateChartFormVisibility(); loadChartColumns(); };
  $("ch-dataset").onchange = loadChartColumns;
  $("ch-sql").onchange = loadChartColumns;
  // Y軸レンジは見た目の微調整なので、入力したらすぐプレビューへ反映する
  $("ch-ymin").onchange = previewChart;
  $("ch-ymax").onchange = previewChart;
  $("ch-xmin").onchange = previewChart;
  $("ch-xmax").onchange = previewChart;
  $("btn-ch-preview").onclick = previewChart;
  $("btn-ch-save").onclick = saveChart;
  $("btn-ch-png").onclick = exportChartPng;
  $("btn-ch-preset").onclick = openPresetModal;
  $("pr-ok").onclick = createYieldPreset;
  $("btn-dash-png").onclick = exportDashPng;
  $("btn-dash-html").onclick = exportDashHtml;
  $("btn-ch-new").onclick = () => {
    editingChartId = null;
    $("ch-name").value = "";
    renderChartList();
  };

  // 分析タブ(インポートモーダル等の他の .subtab を巻き込まないよう分析タブ内に限定)
  document.querySelectorAll("#tab-analytics .subtab").forEach((t) => {
    t.onclick = () => {
      document.querySelectorAll("#tab-analytics .subtab").forEach((x) => x.classList.toggle("active", x === t));
      document.querySelectorAll("#tab-analytics .subpane").forEach((p) => p.classList.toggle("active", p.id === t.dataset.sub));
    };
  });
  $("an-source-kind").onchange = () => {
    const isSql = $("an-source-kind").value === "sql";
    $("an-sql").classList.toggle("hidden", !isSql);
    $("an-dataset").classList.toggle("hidden", isSql);
    anLoadColumns();
  };
  $("an-dataset").onchange = anLoadColumns;
  $("an-sql").onchange = anLoadColumns;
  $("btn-an-profile").onclick = runProfile;
  $("btn-an-reg").onclick = runRegression;
  $("btn-an-clu").onclick = () => runCluster();
  $("btn-clu-elbow").onclick = suggestClusterK;
  $("btn-an-ts").onclick = runTimeseries;
  $("btn-an-tool").onclick = runToolDiff;
  $("btn-an-lot").onclick = runLotTrace;
  $("lot-id").onkeydown = (e) => {
    if (e.key === "Enter") runLotTrace();
  };
  $("btn-clu-save").onclick = () => {
    const name = $("clu-save-name").value.trim();
    if (!name) { setStatus("保存名を入力してください", true); return; }
    runCluster(name);
  };
  $("clu-ax").onchange = drawClusterScatter;
  $("clu-ay").onchange = drawClusterScatter;

  // 自動検定
  $("tst-mode").onchange = () => {
    updateTestModeVisibility();
    $("tst-run-row").classList.add("hidden");
    $("tst-advice").innerHTML = "";
    $("tst-result").innerHTML = "";
  };
  $("btn-tst-advise").onclick = tstAdvise;
  $("btn-tst-run").onclick = tstRun;
  $("btn-tst-md").onclick = tstCopyMarkdown;
  updateTestModeVisibility();

  $("btn-dash-refresh").onclick = () => renderDashboard(true); // 更新はキャッシュも破棄
  $("dash-filter-col").onchange = loadFilterValues;
  $("btn-dash-filter-apply").onclick = applyDashFilter;
  $("btn-project-save").onclick = () => openProjectModal("save");
  $("btn-project-load").onclick = () => openProjectModal("load");
  $("prj-ok").onclick = projectOk;
  $("prj-path").addEventListener("keydown", (e) => { if (e.key === "Enter") projectOk(); });

  // URLハッシュでタブを開く（例: #sql, #dashboard）。ブックマーク／ディープリンク用。
  const openHashTab = () => {
    const h = location.hash.slice(1);
    if (["data", "sql", "charts", "analytics", "dashboard"].includes(h)) switchTab(h);
  };
  window.addEventListener("hashchange", openHashTab);

  updateChartFormVisibility();
  syncSqlHighlight();
  refreshState()
    .then(openHashTab)
    .catch((e) => setStatus(e.message, true));
}

document.addEventListener("DOMContentLoaded", init);
