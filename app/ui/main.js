"use strict";

// Clip content is untrusted text. It is only ever assigned to textContent,
// never to innerHTML, so nothing a user copies can run in this page.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const filter = document.getElementById("filter");
const list = document.getElementById("list");
const preview = document.getElementById("preview");
const meta = document.getElementById("meta");
const hint = document.getElementById("hint");
const count = document.getElementById("count");

const KEYS_HINT = "Enter copy • ⌘P pin • ⌘⌫ delete • Esc close";
const CONFIRM_HINT = "Delete this pinned clip? ⌘⌫ again to confirm, Esc to cancel";

// The last answer from list_clips, or null when it failed.
let listing = null;
let failure = null;
let selectedHash = null;
// Hash of a pinned clip waiting for a second Cmd+Backspace.
let pendingDelete = null;
let listRequest = 0;
let previewRequest = 0;

function rows() {
  return listing ? listing.rows : [];
}

function selectedIndex() {
  return rows().findIndex((row) => row.hash === selectedHash);
}

function selectedRow() {
  return rows()[selectedIndex()] ?? null;
}

async function refresh() {
  const request = ++listRequest;
  let answer = null;
  let error = null;
  try {
    answer = await invoke("list_clips", { query: filter.value });
  } catch (err) {
    error = String(err);
  }
  // A newer keystroke already asked again, so this answer is stale.
  if (request !== listRequest) return;

  listing = answer;
  failure = error;
  if (selectedIndex() === -1) {
    selectedHash = rows().length > 0 ? rows()[0].hash : null;
  }
  if (pendingDelete !== selectedHash) pendingDelete = null;
  renderList();
  renderStatus();
  renderPreview();
}

function renderList() {
  const items = rows().map((row) => {
    const item = document.createElement("li");
    item.setAttribute("role", "option");
    item.dataset.hash = row.hash;
    item.textContent = row.summary === "" ? "(blank)" : row.summary;
    item.classList.toggle("blank", row.summary === "");
    item.classList.toggle("pinned", row.pinned);
    return item;
  });
  if (items.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = failure ?? (filter.value ? "No matches" : "Nothing copied yet");
    items.push(empty);
  }
  list.replaceChildren(...items);
  markSelection();
}

function markSelection() {
  for (const item of list.children) {
    item.classList.toggle("selected", item.dataset.hash === selectedHash);
  }
  list.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
}

function renderStatus() {
  hint.classList.toggle("warn", pendingDelete !== null);
  hint.textContent = pendingDelete !== null ? CONFIRM_HINT : KEYS_HINT;

  if (!listing) {
    count.textContent = "";
  } else if (filter.value) {
    count.textContent = `${listing.matched} of ${listing.total}`;
  } else {
    count.textContent = `${listing.total} clips`;
  }
}

async function renderPreview() {
  const request = ++previewRequest;
  const row = selectedRow();
  if (!row) {
    preview.textContent = "";
    meta.textContent = "";
    return;
  }
  let text = "";
  let details = "";
  try {
    const shown = await invoke("preview_clip", { hash: row.hash });
    const parts = [`${shown.chars.toLocaleString()} characters`];
    if (row.pinned) parts.push("pinned");
    if (shown.truncated) parts.push("showing the first 100 KB");
    text = shown.text;
    details = parts.join(" • ");
  } catch (err) {
    details = String(err);
  }
  if (request !== previewRequest) return;
  preview.textContent = text;
  preview.scrollTop = 0;
  meta.textContent = details;
}

function select(hash) {
  if (hash === selectedHash) return;
  selectedHash = hash;
  pendingDelete = null;
  markSelection();
  renderStatus();
  renderPreview();
}

function move(step) {
  const all = rows();
  if (all.length === 0) return;
  const next = Math.min(Math.max(selectedIndex() + step, 0), all.length - 1);
  select(all[next].hash);
}

async function run(command) {
  const row = selectedRow();
  if (!row) return;
  try {
    await invoke(command, { hash: row.hash });
  } catch (err) {
    console.error(command, err);
  }
  await refresh();
}

async function deleteSelected() {
  const row = selectedRow();
  if (!row) return;
  if (row.pinned && pendingDelete !== row.hash) {
    pendingDelete = row.hash;
    renderStatus();
    return;
  }
  // Keep the cursor in place by moving it to the neighbor before the row goes.
  const index = selectedIndex();
  const neighbor = rows()[index + 1] ?? rows()[index - 1] ?? null;
  try {
    await invoke("delete_clip", { hash: row.hash });
  } catch (err) {
    console.error("delete_clip", err);
  }
  selectedHash = neighbor ? neighbor.hash : null;
  pendingDelete = null;
  await refresh();
}

async function onEscape() {
  if (pendingDelete !== null) {
    pendingDelete = null;
    renderStatus();
  } else if (filter.value !== "") {
    filter.value = "";
    selectedHash = null;
    await refresh();
  } else {
    await invoke("hide_popup");
  }
}

window.addEventListener("keydown", (event) => {
  const command = event.metaKey && !event.ctrlKey && !event.altKey;
  const control = event.ctrlKey && !event.metaKey && !event.altKey;

  if (event.key === "Escape") {
    onEscape();
  } else if (event.key === "ArrowDown" || (control && event.key === "n")) {
    move(1);
  } else if (event.key === "ArrowUp" || (control && event.key === "p")) {
    move(-1);
  } else if (event.key === "Enter") {
    // Cmd+Enter will mean "copy without pasting" once pasting exists.
    run("copy_clip");
  } else if (command && event.key === "p") {
    run("toggle_pin");
  } else if (command && event.key === "Backspace") {
    deleteSelected();
  } else {
    return;
  }
  event.preventDefault();
});

filter.addEventListener("input", () => {
  selectedHash = null;
  refresh();
});

list.addEventListener("click", (event) => {
  const item = event.target.closest("li[data-hash]");
  if (item) select(item.dataset.hash);
  filter.focus();
});

list.addEventListener("dblclick", (event) => {
  if (event.target.closest("li[data-hash]")) run("copy_clip");
});

listen("popup-shown", () => {
  filter.value = "";
  selectedHash = null;
  pendingDelete = null;
  filter.focus();
  refresh();
});

listen("history-changed", refresh);

refresh();
