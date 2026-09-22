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
  return "Hata: " + (typeof e === "string" ? e : "baglanti-hatasi");
}

async function refreshStatus() {
  try {
    const s = await invoke("user_status");
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
    $("ver").textContent = "broker'a ulasilamiyor";
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
    say("Hesap acildi (" + r.account + ").", "ok");
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
  }
});

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

refreshBroker().then(refreshStatus);
