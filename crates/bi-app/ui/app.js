"use strict";
// Kohaku Studio フロントエンド。外部ライブラリ非依存。
// UIはAPIにCommandを投げるだけで、データ処理はすべてRust側で行う。

const $ = (id) => document.getElementById(id);

let datasets = [];
let charts = [];
let currentDataset = null;
let lastSqlResult = null;
let editingChartId = null;

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
  renderHistory(st.queries || []);
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
      <button class="ds-del" title="削除">✕</button>`;
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
      li.textContent = "📁 ..";
      li.onclick = () => browse(r.parent);
      ul.appendChild(li);
    }
    for (const d of r.dirs) {
      const li = document.createElement("li");
      li.textContent = "📁 " + d;
      li.onclick = () => browse(r.path + "\\" + d);
      ul.appendChild(li);
    }
    for (const f of r.files) {
      const li = document.createElement("li");
      const kb = Math.max(1, Math.round(f.size / 1024));
      li.innerHTML = `<span>📄 ${esc(f.name)}</span><span class="fsize">${kb.toLocaleString()} KB</span>`;
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
    series: $("ch-series").value,
    agg: $("ch-agg").value,
    bins: parseInt($("ch-bins").value, 10) || 20,
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
    $("ch-series").value = spec.series || "";
    $("ch-agg").value = spec.agg || "none";
    $("ch-bins").value = spec.bins || 20;
    previewChart();
  });
  renderChartList();
}

function updateChartFormVisibility() {
  const kind = $("ch-source-kind").value;
  const type = $("ch-type").value;
  $("ch-dataset-row").classList.toggle("hidden", kind !== "dataset");
  $("ch-sql-row").classList.toggle("hidden", kind !== "sql");
  $("ch-bins-row").classList.toggle("hidden", type !== "histogram");
  $("ch-y-row").classList.toggle("hidden", type === "histogram" || type === "table");
  $("ch-x-row").classList.toggle("hidden", type === "table");
  $("ch-series-row").classList.toggle("hidden", type === "histogram" || type === "table");
  $("ch-agg-row").classList.toggle("hidden", type === "histogram" || type === "table" || type === "scatter");
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
  for (const id of ["ch-x", "ch-y", "ch-series"]) {
    const sel = $(id);
    const cur = sel.value;
    sel.innerHTML = "";
    if (id === "ch-series") {
      // 系列は任意指定(既定は単一系列)
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
}

function buildChartQuery(spec) {
  const base = chartBaseSql(spec);
  const x = qi(spec.x), y = qi(spec.y);
  // 系列列(任意)。指定時は s 列としてSELECTに含める
  const s = spec.series ? `, ${qi(spec.series)} AS s` : "";
  switch (spec.chart_type) {
    case "table":
      return `SELECT * FROM (${base}) LIMIT 500`;
    case "histogram":
      return `SELECT ${x} AS x FROM (${base}) WHERE ${x} IS NOT NULL LIMIT 100000`;
    case "scatter":
      return `SELECT ${x} AS x, ${y} AS y${s} FROM (${base}) WHERE ${x} IS NOT NULL AND ${y} IS NOT NULL LIMIT 20000`;
    default: {
      if (spec.agg === "none") {
        return `SELECT ${x} AS x, ${y} AS y${s} FROM (${base}) LIMIT 20000`;
      }
      const agg = spec.agg === "count" ? "COUNT(*)" : `${spec.agg.toUpperCase()}(${y})`;
      const grp = spec.series ? `${x}, ${qi(spec.series)}` : x;
      return `SELECT ${x} AS x, ${agg} AS y${s} FROM (${base}) GROUP BY ${grp} ORDER BY ${x} LIMIT 4000`;
    }
  }
}

async function previewChart() {
  const spec = chartSpecFromForm();
  $("chart-title").textContent = spec.name;
  $("chart-msg").textContent = "";
  try {
    const sql = buildChartQuery(spec);
    const r = await api("/api/query", { sql, limit: 100000 });
    drawChartInto($("chart-canvas"), $("chart-table"), spec, r);
  } catch (err) {
    $("chart-msg").textContent = err.message;
  }
}

function drawChartInto(canvas, tableDiv, spec, result) {
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
    li.innerHTML = `<span>${esc(c.name)}</span><button class="ch-del" title="削除">✕</button>`;
    li.querySelector("span").onclick = () => loadChartToForm(c);
    li.querySelector(".ch-del").onclick = async (e) => {
      e.stopPropagation();
      charts = charts.filter((x) => x.id !== c.id);
      dashCache.delete(c.id);
      if (editingChartId === c.id) editingChartId = null;
      await api("/api/charts/set", { charts });
      renderChartList();
    };
    ul.appendChild(li);
  }
}

// ---------- Canvasチャートレンダラ ----------

const CHART_COLORS = { accent: "#4f8ef7", accent2: "#58c9a4", text: "#aab2bf", grid: "#333a46" };

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

function niceTicks(min, max, count) {
  if (!isFinite(min) || !isFinite(max)) return [0, 1];
  if (min === max) { min -= 1; max += 1; }
  const span = max - min;
  const step0 = span / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(step0)));
  let step = mag;
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (mag * m >= step0) { step = mag * m; break; }
  }
  const ticks = [];
  let t = Math.ceil(min / step) * step;
  for (; t <= max + step * 1e-9; t += step) ticks.push(Math.round(t * 1e9) / 1e9);
  // 最終目盛りがデータ最大値を下回るとプロットが枠からはみ出すため、1段追加して覆う
  if (!ticks.length || ticks[ticks.length - 1] < max - step * 1e-9) {
    const base = ticks.length ? ticks[ticks.length - 1] : Math.floor(min / step) * step;
    ticks.push(Math.round((base + step) * 1e9) / 1e9);
  }
  return ticks;
}

function fmtTick(v) {
  if (Math.abs(v) >= 1e6) return (v / 1e6) + "M";
  if (Math.abs(v) >= 1e4) return (v / 1e3) + "k";
  return String(Math.round(v * 1000) / 1000);
}

/** 系列の色パレット(最大8系列) */
const SERIES_COLORS = ["#4f8ef7", "#58c9a4", "#e0a15c", "#e06c75", "#b478e0", "#5cd0e0", "#e0d05c", "#8a94e0"];

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
  const { ctx, w, h } = setupCanvas(canvas);
  const xi = result.columns.indexOf("x");
  const yi = result.columns.indexOf("y");
  const si = result.columns.indexOf("s");
  const C = CHART_COLORS;

  // 系列分解(s列がなければ全行を単一系列として扱う)
  const MAX_SERIES = 8;
  const notes = [];
  let seriesNames = [];
  const bySeries = new Map();
  if (si >= 0) {
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
  const m = { l: 58 + (yLabel ? 16 : 0), r: 16, t: hasLegend ? 34 : 14, b: 52 };
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
      ctx.fillRect(lx, 10, 10, 10);
      ctx.fillStyle = C.text;
      ctx.fillText(lb, lx + 14, 15);
      lx += need;
    }
  };

  const drawNotes = () => {
    if (!notes.length) return;
    ctx.fillStyle = C.text;
    ctx.textAlign = "right";
    ctx.textBaseline = "top";
    ctx.fillText(notes.join(" / "), w - m.r, hasLegend ? 24 : 2);
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

  const noData = () => {
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
  };

  if (spec.chart_type === "histogram") {
    const vals = result.rows.map((r) => Number(r[xi])).filter((v) => isFinite(v));
    if (!vals.length) return noData();
    const lo = Math.min(...vals), hi = Math.max(...vals);
    const nb = Math.max(2, Math.min(200, spec.bins || 20));
    const width = (hi - lo) || 1;
    const counts = new Array(nb).fill(0);
    for (const v of vals) {
      let b = Math.floor(((v - lo) / width) * nb);
      if (b >= nb) b = nb - 1;
      counts[b]++;
    }
    const yMax = Math.max(...counts);
    const yTicks = niceTicks(0, yMax, 5);
    drawAxes(yTicks, 0, yTicks[yTicks.length - 1] || 1);
    const yTop = yTicks[yTicks.length - 1] || 1;
    ctx.fillStyle = C.accent;
    for (let b = 0; b < nb; b++) {
      const bx = m.l + (b / nb) * pw;
      const bw = pw / nb - 1;
      const bh = (counts[b] / yTop) * ph;
      ctx.fillRect(bx, m.t + ph - bh, Math.max(1, bw), bh);
    }
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
    const xMin = Math.min(...xs), xMax = Math.max(...xs);
    const yTicks = niceTicks(Math.min(0, Math.min(...ys)), Math.max(...ys), 5);
    const yMin = yTicks[0], yMax = yTicks[yTicks.length - 1];
    drawAxes(yTicks, yMin, yMax);
    const px = (x) => m.l + ((x - xMin) / ((xMax - xMin) || 1)) * pw;
    const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
    const rad = spec.chart_type === "line" ? (flat.length > 200 ? 0 : 2.5) : Math.max(1.5, 4 - Math.log10(flat.length + 1));
    seriesPts.forEach((pts, k) => {
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
    });
    // X軸
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    if (xNumeric) {
      for (const t of niceTicks(xMin, xMax, 6)) {
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
  const yTicks = niceTicks(Math.min(0, Math.min(...allVals)), Math.max(0, Math.max(...allVals)), 5);
  const yMin = yTicks[0], yMax = yTicks[yTicks.length - 1];
  drawAxes(yTicks, yMin, yMax);
  const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
  const groupW = pw / cats.length;
  const inner = groupW * 0.76;
  const barW = inner / seriesNames.length;
  seriesNames.forEach((n, k) => {
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
  });
  // カテゴリラベル
  ctx.fillStyle = C.text;
  const rotate = cats.length > 8 || cats.some((c) => c.length > 6);
  cats.forEach((c, i) => {
    const cx = m.l + i * groupW + groupW / 2;
    const label = c.length > 14 ? c.slice(0, 14) + "…" : c;
    const stepN = Math.ceil(cats.length / (rotate ? 30 : 15));
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

const CLUSTER_COLORS = ["#4f8ef7", "#58c9a4", "#e0a15c", "#e06c75", "#b478e0", "#5cd0e0", "#e0d05c", "#8a94e0"];
let anColumns = []; // {name, numeric}
let lastCluster = null;
let lastClusterReq = null;

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
    const mark = a.passed ? '<span style="color:#58c9a4">OK</span>' : '<span style="color:#e0a15c">要注意</span>';
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
    return '<div class="hint" style="color:#e0a15c">⚠ 同じ対象のbefore/after比較には、分析タイプ「2つの数値列」→「対応あり」を使用してください。独立群として検定すると誤った結果になります。</div>';
  }
  if (v === "repeated") {
    return '<div class="hint" style="color:#e0a15c">⚠ 同じロット・装置・個体の繰り返し測定は独立ではない可能性があります。検定の前提(独立性)が崩れるため、結果は参考程度に留めてください。</div>';
  }
  if (v === "unknown") {
    return '<div class="hint" style="color:#e0a15c">⚠ サンプルの独立性が不明です。同一対象・同一ロットからの繰り返し測定が含まれる場合、p値は当てになりません。</div>';
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
    const sigColor = t.p_value < (parseFloat($("tst-alpha").value) || 0.05) ? "#58c9a4" : "var(--muted)";
    html += `<div class="interp" style="border-left:3px solid ${sigColor}">${esc(t.interpretation)}</div>`;
    if (t.warnings && t.warnings.length) html += "<ul>" + t.warnings.map((x) => `<li>⚠ ${esc(x)}</li>`).join("") + "</ul>";
    html += groupTable(t.groups);
    // 事後検定(多重比較)
    if (r.posthoc && r.posthoc.pairs) {
      html += `<h4>事後のペアワイズ比較 (${esc(r.posthoc.method)}, 補正: ${esc(r.correction)})</h4>`;
      html += '<div class="table-wrap"><table class="grid"><thead><tr><th>群A</th><th>群B</th><th>統計量</th><th>効果量</th><th>p (未補正)</th><th>p (補正後)</th><th>有意</th></tr></thead><tbody>';
      for (const p of r.posthoc.pairs) {
        const mark = p.significant ? '<span style="color:#58c9a4">✔</span>' : "";
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
  out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/profile", { source: anGetSource() });
    let html = `<div class="hint">${r.n_rows.toLocaleString()}行${r.truncated ? "(上限で打ち切り)" : ""}</div>`;
    // 列統計
    html += "<h4>列статистics</h4>";
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
          const bg = v >= 0 ? `rgba(79,142,247,${alpha})` : `rgba(224,108,117,${alpha})`;
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
    out.innerHTML = html.replace("列статистics", "列統計");
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

// --- 回帰 ---

async function runRegression() {
  const out = $("reg-out");
  const target = $("reg-y").value;
  const features = checkedValues($("reg-x")).filter((f) => f !== target);
  out.innerHTML = '<div class="hint">分析中...</div>';
  $("reg-canvas").classList.add("hidden");
  try {
    const r = await api("/api/analyze/regression", { source: anGetSource(), target, features });
    let html = metricHtml([
      ["決定係数 R²", fmtNum(r.r2, 4)],
      ["自由度調整済み R²", fmtNum(r.adj_r2, 4)],
      ["RMSE", fmtNum(r.rmse, 4)],
      ["サンプル数", r.n.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped}`  + ")" : "")],
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
    out.innerHTML = html;

    const canvas = $("reg-canvas");
    canvas.classList.remove("hidden");
    if (r.single_feature) {
      const pts = r.points.map((p) => ({ x: p[0], y: p[1] }));
      drawAnScatter(canvas, pts, {
        xlab: r.feature, ylab: r.target,
        line: { slope: r.coef[1], intercept: r.coef[0] },
      });
    } else {
      const pts = r.points.map((p) => ({ x: p[0], y: p[1] }));
      drawAnScatter(canvas, pts, { xlab: "実測値", ylab: "予測値", diag: true });
    }
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

// --- クラスタリング ---

async function runCluster(saveAs) {
  const out = $("clu-out");
  const features = checkedValues($("clu-x"));
  const k = parseInt($("clu-k").value, 10) || 3;
  const req = { source: anGetSource(), features, k };
  if (saveAs) req.save_as = saveAs;
  if (!saveAs) out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/cluster", req);
    lastCluster = r;
    lastClusterReq = { source: req.source, features, k };
    let html = metricHtml([
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

function drawClusterScatter() {
  if (!lastCluster) return;
  const xi = parseInt($("clu-ax").value, 10) || 0;
  const yi = parseInt($("clu-ay").value, 10) || 0;
  const canvas = $("clu-canvas");
  canvas.classList.remove("hidden");
  const nf = lastCluster.features.length;
  const pts = lastCluster.points.map((p) => ({ x: p[xi], y: p[yi], c: p[nf] }));
  drawAnScatter(canvas, pts, {
    xlab: lastCluster.features[xi],
    ylab: lastCluster.features[yi],
    colored: true,
  });
}

// 汎用散布図(回帰線・対角線・クラスタ色分け対応)
function drawAnScatter(canvas, pts, opts) {
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
  for (const p of valid) {
    ctx.fillStyle = opts.colored
      ? CLUSTER_COLORS[(p.c || 0) % CLUSTER_COLORS.length] + "b0"
      : "rgba(79,142,247,0.55)";
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
  for (const spec of charts) {
    if (seq !== dashRenderSeq) return; // 新しい描画に取って代わられたら中断
    const card = document.createElement("div");
    card.className = "dash-card";
    applyCardLayout(card, spec);
    const head = document.createElement("div");
    head.className = "dash-head";
    head.innerHTML = `<h4>${esc(spec.name)}</h4>
      <button class="dash-btn" data-act="left" title="左へ移動">◀</button>
      <button class="dash-btn" data-act="right" title="右へ移動">▶</button>
      <button class="dash-btn" data-act="width" title="幅を切替(1列 / 2列)">⬌</button>
      <button class="dash-btn" data-act="height" title="高さを切替(小 / 中 / 大)">↕</button>`;
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
        r = await api("/api/query", { sql: buildChartQuery(spec), limit: 100000 });
        dashCache.set(spec.id, r);
      }
      if (seq !== dashRenderSeq) return;
      drawChartInto(canvas, tdiv, spec, r);
    } catch (err) {
      card.innerHTML += `<div class="hint error">${esc(err.message)}</div>`;
    }
  }
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
  $("sql-history").onchange = () => {
    if ($("sql-history").value) $("sql-input").value = $("sql-history").value;
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

  $("ch-type").onchange = updateChartFormVisibility;
  $("ch-source-kind").onchange = () => { updateChartFormVisibility(); loadChartColumns(); };
  $("ch-dataset").onchange = loadChartColumns;
  $("ch-sql").onchange = loadChartColumns;
  $("btn-ch-preview").onclick = previewChart;
  $("btn-ch-save").onclick = saveChart;
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
  refreshState()
    .then(openHashTab)
    .catch((e) => setStatus(e.message, true));
}

document.addEventListener("DOMContentLoaded", init);
