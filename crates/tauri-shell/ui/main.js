"use strict";
// whisperexe ana pencere surucusu. Komutlar Rust tarafinda; sirlar JS'te
// tutulmaz (jetonlar Rust belleginde). Tum metinler kisa, hata = broker kodu.
const invoke = window.__TAURI__.core.invoke;

const $ = (id) => document.getElementById(id);
const msgEl = $("msg");

function say(text, kind) {
  msgEl.textContent = text || "";
  msgEl.className = "msg" + (kind ? " " + kind : "");
}

function tl(kurus) {
  return (kurus / 100).toFixed(2) + " TL";
}

function fmtErr(e) {
  return "Hata: " + (typeof e === "string" ? e : "bağlantı-hatası");
}

async function refreshStatus() {
  try {
    let s = await invoke("user_status");
    // Access bitmis ama refresh duruyorsa sessizce yenile (beni hatirla).
    if (s.logged_in && !s.access_valid) {
      try {
        await invoke("user_refresh");
        s = await invoke("user_status");
      } catch (e) {
        // Refresh de bitmis/hatali: giris formuna dus (donma YOK).
      }
    }
    $("broker").textContent = s.broker || "—";
    const dot = $("netdot");
    if (s.logged_in) {
      $("loggedout").hidden = true;
      $("loggedin").hidden = false;
      $("acc").textContent = s.account || "—";
      const h = (s.hwid || "").slice(0, 13);
      $("hwid").textContent = h ? h + "…" : "—";
      dot.className = "dot " + (s.access_valid ? "ok" : "warn");
      if (s.balance_kurus !== null && s.balance_kurus !== undefined) {
        paintBalance(s.balance_kurus, null, null);
      }
      await refreshMe(true);
    } else {
      $("loggedout").hidden = false;
      $("loggedin").hidden = true;
      dot.className = "dot idle";
      $("balance").textContent = "—";
    }
  } catch (e) {
    $("netdot").className = "dot err";
    say(fmtErr(e), "err");
  }
}

function paintBalance(kurus, homeRate, fbRate) {
  $("balance").textContent = tl(kurus);
  const low = $("lowbadge");
  if (homeRate && fbRate) {
    const mev = kurus / 100 / (homeRate / 100);
    const mfb = kurus / 100 / (fbRate / 100);
    $("minev").textContent = "ev: ~" + Math.floor(mev) + " dk";
    $("minfb").textContent = "fallback: ~" + Math.floor(mfb) + " dk";
    const bad = mev < 10 || mfb < 10;
    low.hidden = !bad;
  } else {
    low.hidden = true;
  }
}

async function refreshMe(quiet) {
  try {
    const v = await invoke("user_me");
    paintBalance(v.balance_kurus, v.home_krs_per_min / 1, v.fallback_krs_per_min / 1);
    if (!quiet) say("Tazelendi.", "ok");
  } catch (e) {
    if (!quiet) say(fmtErr(e), "err");
  }
}

async function refreshAppVersion() {
  try {
    const v = await invoke("app_version");
    if (v && v.version) $("appver").textContent = "v" + v.version;
  } catch (e) {
    // sessiz: rozet boş kalır, toast yok
  }
}

async function refreshBroker() {
  try {
    const v = await invoke("broker_info");
    $("ver").textContent = "sürüm " + (v.version || "?");
    $("bver").textContent = v.version || "—";
    $("bfloor").textContent = v.floor_version || "(taban yok)";
    const notes = v.notes || "";
    $("upd").textContent = notes;
    $("netdot").className = "dot ok";
  } catch (e) {
    $("ver").textContent = "broker'a ulaşılamıyor";
    $("bver").textContent = "—";
    $("netdot").className = "dot err";
    say(fmtErr(e), "err");
  }
}

$("loginform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  say("");
  $("loginbtn").disabled = true;
  try {
    const r = await invoke("user_login", {
      account: $("li-acc").value.trim(),
      password: $("li-pw").value,
    });
    $("li-pw").value = "";
    say("Girdin (" + r.account + ").", "ok");
    await refreshStatus();
  } catch (e) {
    say(fmtErr(e), "err");
  } finally {
    $("loginbtn").disabled = false;
  }
});

$("redeemform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  say("");
  $("redeembtn").disabled = true;
  try {
    const r = await invoke("user_redeem", {
      code: $("rd-code").value.trim(),
      username: $("rd-user").value.trim(),
      password: $("rd-pw").value,
    });
    $("rd-pw").value = "";
    $("rd-code").value = "";
    say("Hesap açıldı (" + r.account + ").", "ok");
    await refreshStatus();
  } catch (e) {
    say(fmtErr(e), "err");
  } finally {
    $("redeembtn").disabled = false;
  }
});

$("mebtn").addEventListener("click", () => refreshMe(false));
$("updbtn").addEventListener("click", async () => {
  $("updbtn").disabled = true;
  say("İndiriliyor…");
  try {
    const r = await invoke("fetch_update");
    say("Kurulum başladı (" + (r.tag || "?") + ").", "ok");
  } catch (e) {
    say(fmtErr(e), "err");
    $("updbtn").disabled = false;
  }
});
$("logoutbtn").addEventListener("click", async () => {
  await invoke("user_logout");
  say("");
  refreshStatus();
});
$("adminbtn").addEventListener("click", async () => {
  try {
    await invoke("open_admin");
  } catch (e) {
    say(fmtErr(e), "err");
    return;
  }
  selectPage("admin");
});

// Sayfalar (Durum / Yonetim; mevcut selectTab deseni buyutuldu).
function selectPage(which) {
  const status = which !== "admin";
  $("page-status").hidden = !status;
  $("page-admin").hidden = status;
  $("wrap").classList.toggle("wide", !status);
  $("tabbtn-page-status").classList.toggle("on", status);
  $("tabbtn-page-admin").classList.toggle("on", !status);
  $("tabbtn-page-status").setAttribute("aria-selected", String(status));
  $("tabbtn-page-admin").setAttribute("aria-selected", String(!status));
}
$("tabbtn-page-status").addEventListener("click", () => selectPage("status"));
$("tabbtn-page-admin").addEventListener("click", () => selectPage("admin"));

// Sekmeler (yalnizca gorunum; giris/davet mantigi degismez).
function selectTab(which) {
  const login = which !== "redeem";
  $("tab-login").hidden = !login;
  $("tab-redeem").hidden = login;
  $("tabbtn-login").classList.toggle("on", login);
  $("tabbtn-redeem").classList.toggle("on", !login);
  $("tabbtn-login").setAttribute("aria-selected", String(login));
  $("tabbtn-redeem").setAttribute("aria-selected", String(!login));
}
$("tabbtn-login").addEventListener("click", () => selectTab("login"));
$("tabbtn-redeem").addEventListener("click", () => selectTab("redeem"));

// ---- yonetim sayfasi (admin.js'den tasindi; id'ler adm- onekli, invoke
// adlari ve argumanlari aynen korunur). Anahtar/sifre degerleri JS'te
// tutulmaz: forma yazilir, komuta verilir, alan hemen temizlenir.
const admMsgEl = $("adm-msg");
let admDetailUser = null;

function admSay(text, kind) {
  admMsgEl.textContent = text || "";
  admMsgEl.className = "msg" + (kind ? " " + kind : "");
}

// Sayisal alan dogrulama: bos -> null (opsiyonel), bozuk -> NaN degil hata.
// Rust'a NaN gonderilmez (JSON'da null olur, yanlis is yapar).
function admInt(raw, { min = null, allowEmpty = false } = {}) {
  const s = String(raw ?? "").trim();
  if (s === "") return allowEmpty ? null : NaN;
  if (!/^-?\d+$/.test(s)) return NaN;
  const n = parseInt(s, 10);
  if (!Number.isSafeInteger(n)) return NaN;
  if (min !== null && n < min) return NaN;
  return n;
}

document.querySelectorAll("nav.rail button").forEach((b) => {
  b.addEventListener("click", () => {
    document.querySelectorAll("nav.rail button").forEach((x) => x.classList.remove("on"));
    document.querySelectorAll(".pane").forEach((x) => x.classList.remove("on"));
    b.classList.add("on");
    $("adm-pane-" + b.dataset.pane).classList.add("on");
    admSay("");
  });
});

$("adm-gateform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("adm-gatemsg").textContent = "";
  $("adm-gatebtn").disabled = true;
  try {
    await invoke("admin_login", { password: $("adm-apw").value });
    $("adm-apw").value = "";
    $("adm-gate").hidden = true;
    $("adm-app").hidden = false;
    $("adm-logoutbtn").hidden = false;
    $("adm-adot").className = "dot ok";
    $("adm-asub").textContent = "açık";
    admLoadUsers();
  } catch (e) {
    $("adm-gatemsg").textContent = fmtErr(e);
    $("adm-gatemsg").className = "msg err";
  } finally {
    $("adm-gatebtn").disabled = false;
  }
});

$("adm-logoutbtn").addEventListener("click", async () => {
  await invoke("admin_logout");
  $("adm-app").hidden = true;
  $("adm-gate").hidden = false;
  $("adm-logoutbtn").hidden = true;
  $("adm-adot").className = "dot idle";
  $("adm-asub").textContent = "kilitli";
});

// ---- hesaplar ----
async function admLoadUsers() {
  const tb = $("adm-userstable").querySelector("tbody");
  try {
    const v = await invoke("admin_users");
    const rows = v.users || [];
    tb.textContent = "";
    if (!rows.length) {
      const tr = document.createElement("tr");
      const td = document.createElement("td");
      td.colSpan = 6;
      td.className = "muted";
      td.textContent = "hesap yok — Davet sekmesinden aç.";
      tr.appendChild(td);
      tb.appendChild(tr);
      return;
    }
    for (const r of rows) {
      const tr = document.createElement("tr");
      const cUser = document.createElement("td");
      cUser.textContent = r.username;
      const cBal = document.createElement("td");
      cBal.className = "num";
      cBal.textContent = tl(r.balance_krs);
      const cHome = document.createElement("td");
      cHome.className = "num";
      cHome.textContent = r.usage_home_secs;
      const cFb = document.createElement("td");
      cFb.className = "num";
      cFb.textContent = r.usage_fb_secs;
      const cState = document.createElement("td");
      const badge = document.createElement("span");
      if (r.suspended) {
        badge.className = "badge err";
        badge.textContent = "durduruldu";
      } else if (r.low_balance) {
        badge.className = "badge warn";
        badge.textContent = "düşük";
      } else {
        badge.className = "badge ok";
        badge.textContent = "açık";
      }
      cState.appendChild(badge);
      const cAct = document.createElement("td");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "ghost";
      btn.textContent = "Aç";
      btn.addEventListener("click", () => admOpenDetail(r.username));
      cAct.appendChild(btn);
      tr.append(cUser, cBal, cHome, cFb, cState, cAct);
      tb.appendChild(tr);
    }
    admSay("");
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
}

async function admOpenDetail(username) {
  admDetailUser = username;
  try {
    const v = await invoke("admin_user", { username });
    $("adm-detail").hidden = false;
    $("adm-dname").textContent = v.username;
    $("adm-dmeta").textContent =
      "bakiye " + tl(v.balance_krs) + " · cihaz " + v.devices +
      " · ev-tek " + (v.home_only ? "acik" : "kapali");
    $("adm-detail").scrollIntoView({ block: "nearest" });
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
}

$("adm-usersbtn").addEventListener("click", admLoadUsers);

$("adm-topupform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!admDetailUser) return;
  const amount_krs = admInt($("adm-tp-amt").value, { min: 1 });
  if (!Number.isSafeInteger(amount_krs)) { admSay("Hata: tutar-gecersiz", "err"); return; }
  try {
    const v = await invoke("admin_topup", {
      username: admDetailUser,
      amountKrs: amount_krs,
    });
    $("adm-tp-amt").value = "";
    admSay(admDetailUser + " yeni bakiye " + tl(v.balance_krs) + ".", "ok");
    admLoadUsers();
    admOpenDetail(admDetailUser);
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-deductform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!admDetailUser) return;
  const amount_krs = admInt($("adm-dc-amt").value, { min: 1 });
  if (!Number.isSafeInteger(amount_krs)) { admSay("Hata: tutar-gecersiz", "err"); return; }
  try {
    const v = await invoke("admin_deduct", {
      username: admDetailUser,
      amountKrs: amount_krs,
    });
    $("adm-dc-amt").value = "";
    admSay(admDetailUser + " yeni bakiye " + tl(v.balance_krs) + ".", "ok");
    admLoadUsers();
    admOpenDetail(admDetailUser);
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-capform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!admDetailUser) return;
  const raw = $("adm-cp-cap").value.trim();
  const fb_cap = admInt(raw, { min: 0, allowEmpty: true });
  if (raw !== "" && !Number.isSafeInteger(fb_cap)) { admSay("Hata: tavan-gecersiz", "err"); return; }
  try {
    await invoke("admin_limits", {
      username: admDetailUser,
      dailyHome: null, dailyFb: null, monthlyHome: null, monthlyFb: null,
      fbCap: fb_cap,
      homeOnly: null,
    });
    $("adm-cp-cap").value = "";
    admSay("Tavan yazıldı.", "ok");
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-homeonlybtn").addEventListener("click", async () => {
  if (!admDetailUser) return;
  try {
    const cur = await invoke("admin_user", { username: admDetailUser });
    await invoke("admin_home_only", { username: admDetailUser, homeOnly: !cur.home_only });
    admSay("Sadece-ev " + (!cur.home_only ? "açıldı." : "kapatıldı."), "ok");
    admOpenDetail(admDetailUser);
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-hwidbtn").addEventListener("click", async () => {
  if (!admDetailUser) return;
  try {
    await invoke("admin_hwid_reset", { username: admDetailUser });
    admSay("Cihaz slotları temizlendi.", "ok");
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-suspendbtn").addEventListener("click", async () => {
  if (!admDetailUser) return;
  try {
    const cur = await invoke("admin_user", { username: admDetailUser });
    const stop = !cur.suspended;
    await invoke("admin_suspend", { username: admDetailUser, stop });
    admSay(stop ? "Hesap durduruldu." : "Hesap açıldı.", "ok");
    admLoadUsers();
    admOpenDetail(admDetailUser);
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

// ---- davet ----
$("adm-inviteform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const opening_krs = admInt($("adm-iv-bal").value, { min: 0 });
  if (!Number.isSafeInteger(opening_krs)) { admSay("Hata: bakiye-gecersiz", "err"); return; }
  try {
    const v = await invoke("admin_invite", {
      username: $("adm-iv-user").value.trim(),
      openingKrs: opening_krs,
      code: $("adm-iv-code").value.trim() === "" ? null : $("adm-iv-code").value.trim(),
    });
    $("adm-codewrap").hidden = false;
    $("adm-codebig").textContent = v.code;
    admSay("Davet hazır (" + v.username + ").", "ok");
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

// ---- tarifeler ----
async function admLoadTariffs() {
  try {
    const v = await invoke("admin_tariffs");
    const fb = v.fallback_type === "fixed_krs_per_min"
      ? tl(v.fallback_value) + "/dk sabit"
      : (v.fallback_value / 100).toFixed(0) + "x carpan";
    $("adm-tariffcur").textContent =
      "v" + v.version + " · ev " + tl(v.home_krs_per_min) + "/dk · fallback " + fb +
      " · taban " + v.upstream_min_secs + "sn";
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
}
$("adm-tariffsbtn").addEventListener("click", admLoadTariffs);

$("adm-tariffform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const home_krs_per_min = admInt($("adm-tf-home").value, { min: 0 });
  const fx = $("adm-tf-fixed").value.trim();
  const bp = $("adm-tf-bp").value.trim();
  const fallback_fixed = admInt(fx, { min: 0, allowEmpty: true });
  const fallback_bp = admInt(bp, { min: 1, allowEmpty: true });
  if (!Number.isSafeInteger(home_krs_per_min)) { admSay("Hata: tarife-gecersiz", "err"); return; }
  if (fx !== "" && !Number.isSafeInteger(fallback_fixed)) { admSay("Hata: sabit-gecersiz", "err"); return; }
  if (bp !== "" && !Number.isSafeInteger(fallback_bp)) { admSay("Hata: carpan-gecersiz", "err"); return; }
  try {
    await invoke("admin_set_tariff", {
      homeKrsPerMin: home_krs_per_min,
      fallbackFixed: fallback_fixed,
      fallbackBp: fallback_bp,
    });
    admSay("Tarife sonraki isteklere uygulanır.", "ok");
    admLoadTariffs();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

// ---- fallback ----
function admPaintVendor(v) {
  const localTaban = v.local ? v.local.upstream_min_secs : 3;
  $("adm-vendorcur").textContent =
    "hat " + v.vendor + " · v" + v.version +
    " · groq taban " + v.groq.upstream_min_secs + "sn" +
    " · openai taban " + v.openai.upstream_min_secs + "sn" +
    " · local taban " + localTaban + "sn (ücretsiz)";
  $("adm-kgroq").className = "dot " + (v.groq.key_set ? "ok" : "idle");
  $("adm-kopenai").className = "dot " + (v.openai.key_set ? "ok" : "idle");
  const lc = $("adm-localcur");
  if (lc) lc.textContent = "local: 0,00 TL/dk · ücretsiz · anahtar gerekmez · taban " + localTaban + "sn";
}

async function admLoadVendor() {
  try {
    const v = await invoke("admin_vendor");
    admPaintVendor(v);
    const s = await invoke("admin_switch");
    $("adm-swbtn").textContent = "Şalter: " + (s.open ? "açık" : "kapalı");
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
}
$("adm-vendorbtn").addEventListener("click", admLoadVendor);

$("adm-swbtn").addEventListener("click", async () => {
  try {
    const s = await invoke("admin_switch");
    await invoke("admin_set_switch", { open: !s.open });
    admSay(!s.open ? "Fallback açıldı." : "Fallback durduruldu (yeni işler bakım retli).", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-vgroq").addEventListener("click", async () => {
  try {
    await invoke("admin_set_vendor", { vendor: "groq" });
    admSay("Hat groq.", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-vopenai").addEventListener("click", async () => {
  try {
    await invoke("admin_set_vendor", { vendor: "openai" });
    admSay("Hat openai.", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-vlocal").addEventListener("click", async () => {
  try {
    await invoke("admin_set_vendor", { vendor: "local" });
    admSay("Hat local (ücretsiz).", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-vpriceform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const fx = $("adm-vp-fixed").value.trim();
  const bp = $("adm-vp-bp").value.trim();
  const fixed = admInt(fx, { min: 0, allowEmpty: true });
  const bpv = admInt(bp, { min: 1, allowEmpty: true });
  if (fx !== "" && !Number.isSafeInteger(fixed)) { admSay("Hata: sabit-gecersiz", "err"); return; }
  if (bp !== "" && !Number.isSafeInteger(bpv)) { admSay("Hata: carpan-gecersiz", "err"); return; }
  try {
    await invoke("admin_vendor_price", {
      vendor: $("adm-vp-vendor").value,
      fixed,
      bp: bpv,
    });
    admSay("Hat fiyatı yazıldı.", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-vsecbtn").addEventListener("click", async () => {
  const secs = admInt($("adm-vp-secs").value, { min: 3 });
  if (!Number.isSafeInteger(secs)) { admSay("Hata: taban-gecersiz (en az 3)", "err"); return; }
  try {
    await invoke("admin_vendor_upstream", {
      vendor: $("adm-vp-vendor").value,
      secs,
    });
    admSay("Hat tabanı yazıldı.", "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

async function admKeySet(vendor, inputId) {
  try {
    const r = await invoke("admin_key_set", { vendor, key: $(inputId).value });
    $(inputId).value = "";
    admSay(vendor + " anahtar " + (r.stored ? "girildi." : "temizlendi."), "ok");
    admLoadVendor();
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
}
$("adm-keygroq").addEventListener("submit", (ev) => { ev.preventDefault(); admKeySet("groq", "adm-kg-in"); });
$("adm-keyopenai").addEventListener("submit", (ev) => { ev.preventDefault(); admKeySet("openai", "adm-ko-in"); });
$("adm-kg-clear").addEventListener("click", async () => {
  try {
    await invoke("admin_key_clear", { vendor: "groq" });
    admSay("groq anahtar silindi.", "ok");
    admLoadVendor();
  } catch (e) { admSay(fmtErr(e), "err"); }
});
$("adm-ko-clear").addEventListener("click", async () => {
  try {
    await invoke("admin_key_clear", { vendor: "openai" });
    admSay("openai anahtar silindi.", "ok");
    admLoadVendor();
  } catch (e) { admSay(fmtErr(e), "err"); }
});

// ---- denetim + diskler ----
$("adm-auditbtn").addEventListener("click", async () => {
  try {
    const v = await invoke("admin_audit");
    const list = $("adm-auditlist");
    list.textContent = "";
    const rows = (v.entries || []).slice().reverse();
    if (!rows.length) list.textContent = "kayıt yok";
    for (const e of rows.slice(0, 200)) {
      const d = document.createElement("div");
      const t = new Date(e.at * 1000).toLocaleString("tr-TR");
      d.textContent = t + " · " + e.admin + " · " + e.action + " · " + e.detail;
      list.appendChild(d);
    }
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

$("adm-disksbtn").addEventListener("click", async () => {
  try {
    const v = await invoke("admin_disks");
    $("adm-diskscur").textContent = (v.disks || [])
      .map((d) => d.disk + " %" + d.used_pct + (d.warn ? " (UYARI)" : ""))
      .join(" · ") || "—";
  } catch (e) {
    admSay(fmtErr(e), "err");
  }
});

// ---- kisayol (bas-konus tusu; Rust hotkey.json'da tutar) ----
function paintHotkey(k) {
  const key = typeof k === "string" && k ? k : "F9";
  $("hk-input").value = key;
  $("f9key").textContent = key;
}

async function loadHotkey() {
  try {
    const r = await invoke("hotkeyGet");
    paintHotkey(r.hotkey);
  } catch (e) {
    paintHotkey("F9");
  }
}

$("hkform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("hksave").disabled = true;
  try {
    const r = await invoke("hotkeySet", { key: $("hk-input").value });
    paintHotkey(r.hotkey);
    $("hkmsg").textContent = "Kaydedildi (" + r.hotkey + ").";
    $("hkmsg").className = "msg ok";
  } catch (e) {
    $("hkmsg").textContent = fmtErr(e);
    $("hkmsg").className = "msg err";
  } finally {
    $("hksave").disabled = false;
  }
});

// ---- ses paneli (mikrofon sec + seviye testi; F9 akisina dokunmaz) ----
const MIC_KEY = "whisper_mic_id";
const SILENCE_RMS = 0.015;
const WARN_RMS = 0.03;

function micSay(text, kind) {
  const el = $("micmsg");
  el.textContent = text || "";
  el.className = "msg" + (kind ? " " + kind : "");
}

function micLevel(text) {
  $("miclevel").textContent = text || "";
}

async function micList() {
  const sel = $("miclist");
  if (!navigator.mediaDevices || !navigator.mediaDevices.enumerateDevices) {
    sel.textContent = "";
    micSay("Hata: mikrofon-izin-yok", "err");
    return;
  }
  try {
    const devs = await navigator.mediaDevices.enumerateDevices();
    const inputs = devs.filter((d) => d.kind === "audioinput");
    sel.textContent = "";
    if (!inputs.length) {
      micSay("Hata: mikrofon-izin-yok", "err");
      return;
    }
    const saved = localStorage.getItem(MIC_KEY) || "";
    let found = false;
    inputs.forEach((d, i) => {
      const o = document.createElement("option");
      o.value = d.deviceId || "";
      o.textContent = d.label || ("Mikrofon " + (i + 1));
      if (saved && d.deviceId === saved) {
        o.selected = true;
        found = true;
      }
      sel.appendChild(o);
    });
    if (saved && !found && inputs[0].label) {
      // Etiketler izinsiz bos gelir; izin sonrasi liste tazelenir.
    }
    const anyLabel = inputs.some((d) => d.label);
    if (!anyLabel) micSay("Hata: mikrofon-izin-yok", "err");
    else micSay("", "");
  } catch (e) {
    micSay("Hata: mikrofon-izin-yok", "err");
  }
}

if ($("miclist")) {
  $("miclist").addEventListener("change", (ev) => {
    try {
      localStorage.setItem(MIC_KEY, ev.target.value || "");
    } catch (e) {
      // sessiz: hatirlama zorunlu degil
    }
  });
}
if ($("micrefresh")) $("micrefresh").addEventListener("click", micList);

if ($("mictest")) {
  $("mictest").addEventListener("click", async () => {
    micSay("");
    micLevel("dinleniyor…");
    $("mictest").disabled = true;
    let stream = null;
    let ctx = null;
    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw new Error("no-mic");
      }
      const saved = localStorage.getItem(MIC_KEY) || "";
      const selId = ($("miclist") && $("miclist").value) || saved || "";
      const audio = selId ? { deviceId: { exact: selId } } : true;
      stream = await navigator.mediaDevices.getUserMedia({ audio });
      ctx = new (window.AudioContext || window.webkitAudioContext)();
      const src = ctx.createMediaStreamSource(stream);
      const an = ctx.createAnalyser();
      an.fftSize = 2048;
      src.connect(an);
      const buf = new Float32Array(an.fftSize);
      let peak = 0;
      const t0 = Date.now();
      while (Date.now() - t0 < 3000) {
        an.getFloatTimeDomainData(buf);
        let sum = 0;
        for (let i = 0; i < buf.length; i++) sum += buf[i] * buf[i];
        const rms = Math.sqrt(sum / buf.length);
        if (rms > peak) peak = rms;
        micLevel("seviye: " + peak.toFixed(3));
        await new Promise((r) => setTimeout(r, 120));
      }
      if (peak < SILENCE_RMS) {
        micSay("Ses duyulmuyor — mikrofonu/kazancı kontrol et (ucret yok)", "warn");
      } else if (peak < WARN_RMS) {
        micSay("Ses düşük", "warn");
      } else {
        micSay("Mikrofon sağlam", "ok");
      }
      micLevel("seviye: " + peak.toFixed(3));
    } catch (e) {
      micSay("Hata: mikrofon-izin-yok", "err");
      micLevel("");
    } finally {
      if (stream) {
        stream.getTracks().forEach((t) => { try { t.stop(); } catch (e) {} });
      }
      if (ctx) {
        try { await ctx.close(); } catch (e) {}
      }
      $("mictest").disabled = false;
      micList();
    }
  });
}
if ($("miclist")) micList();

refreshAppVersion();
refreshBroker().then(refreshStatus).then(loadHotkey);
