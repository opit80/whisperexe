"use strict";
// whisperexe overlay sürücüsü (WebView2 + tarayıcı önizleme).
// Tauri: Rust tarafı emit("overlay", OverlayView JSON) eder; withGlobalTauri
// açıkken window.__TAURI__.event.listen ile bağlanır.
// Tarayıcı önizleme: window.applyState(view) aynen çalışır; açılış hidden.
const pill = document.getElementById("pill");
const dot = document.getElementById("dot");
const secs = document.getElementById("secs");
const label = document.getElementById("label");
const badge = document.getElementById("badge");
const toast = document.getElementById("toast");
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

const LABELS = {
  "hidden": "hazır", "recording": "kayıt", "sending": "gönderiliyor",
  "queued": "kuyruk", "done": "tamam",
  "update-pending": "yeniden başlatmada güncellenecek",
  "update-blocked": "devam için güncelle"
};

// Tauri: emit("overlay", OverlayView JSON). Kabuk tarafı render() çıktısı.
function applyState(v) {
  document.body.dataset.state = v.state;
  pill.dataset.state = v.state;
  dot.classList.toggle("pulse", !!v.pulse && !reduced);
  const m = Math.floor(v.secs / 60), s = v.secs % 60;
  secs.textContent = m + ":" + String(s).padStart(2, "0");
  secs.style.display = v.state === "recording" ? "" : "none";
  label.textContent = (v.state === "queued" && v.queue)
    ? "kuyruk " + v.queue.position + "/" + v.queue.pending
    : (LABELS[v.state] || v.state);
  if (v.low_mic && v.state === "recording") label.textContent += " · mikrofonu kontrol et";
  if (v.low_balance) {
    badge.hidden = false;
    badge.textContent = "bakiye ~" + Math.floor(v.low_balance.minutes_left) + " dk";
  } else badge.hidden = true;
  if (v.toast) { toast.hidden = false; toast.textContent = v.toast.text; }
  else toast.hidden = true;
}
window.applyState = applyState;

// Tauri kabuğunda canlı dinle; tarayıcıda tanımsızdır (önizleme bozulmaz).
if (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen) {
  window.__TAURI__.event.listen("overlay", (e) => applyState(e.payload));
}
