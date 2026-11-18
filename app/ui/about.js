"use strict";

const { invoke } = window.__TAURI__.core;

function text(id, value) {
  document.getElementById(id).textContent = value;
}

function open(link) {
  invoke("open_link", { link }).catch((err) => console.error(err));
}

document.getElementById("repo").addEventListener("click", () => open("repo"));
document.getElementById("data-folder").addEventListener("click", () => open("data_folder"));
document.getElementById("licenses").addEventListener("click", () => open("licenses"));

invoke("about_info")
  .then((info) => {
    text("name", info.name);
    text("version", `Version ${info.version}`);
    text("description", info.description);
    text("repo", info.repo.replace(/^https:\/\//, ""));
    text("data-folder", info.data_folder);
    text("copyright", info.copyright);
    text("license", info.license);
    document.getElementById("licenses").hidden = !info.has_licenses;
  })
  .catch((err) => text("description", String(err)));
