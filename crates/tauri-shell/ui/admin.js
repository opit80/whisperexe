"use strict";
// whisperexe yonetim surucusu. Anahtar/sifre degerleri JS'te tutulmaz:
// forma yazilir, komuta verilir, alan hemen temizlenir. Yanitlarda yalnizca
// VAR/YOK doner; deger ekrana hic yazilmaz.
const invoke = window.__TAURI__.core.invoke;

const $ = (id) => document.getElementById(id);
const msgEl = $("msg");
let detailUser = null;

function say(text, kind) {
  msgEl.textContent = text || "";
  msgEl.className = "msg" + (kind ? " " + kind : "");
}
function fmtErr(e) {
  return "Hata: " + (typeof e === "string" ? e : "baglanti-hatasi");
}
function tl(k) {
  return (k / 100).toFixed(2) + " TL";
}

document.querySelectorAll("nav.rail button").forEach((b) => {
  b.addEventListener("click", () => {
    document.querySelectorAll("nav.rail button").forEach((x) => x.classList.remove("on"));
    document.querySelectorAll(".pane").forEach((x) => x.classList.remove("on"));
    b.classList.add("on");
    $("pane-" + b.dataset.pane).classList.add("on");
    say("");
  });
});

$("gateform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("gatemsg").textContent = "";
  $("gatebtn").disabled = true;
  try {
    await invoke("admin_login", { password: $("apw").value });
    $("apw").value = "";
    $("gate").hidden = true;
    $("app").hidden = false;
    $("logoutbtn").hidden = false;
    $("adot").className = "dot ok";
    $("asub").textContent = "acik";
    loadUsers();
  } catch (e) {
    $("gatemsg").textContent = fmtErr(e);
    $("gatemsg").className = "msg err";
  } finally {
    $("gatebtn").disabled = false;
  }
});

$("logoutbtn").addEventListener("click", async () => {
  await invoke("admin_logout");
  $("app").hidden = true;
  $("gate").hidden = false;
  $("logoutbtn").hidden = true;
  $("adot").className = "dot idle";
  $("asub").textContent = "kilitli";
});

// ---- hesaplar ----
async function loadUsers() {
  const tb = $("userstable").querySelector("tbody");
  try {
    const v = await invoke("admin_users");
    const rows = v.users || [];
    if (!rows.length) {
      tb.innerHTML = '<tr><td colspan="6" class="muted">hesap yok — Davet sekmesinden ac.</td></tr>';
      return;
    }
    tb.innerHTML = "";
    for (const r of rows) {
      const tr = document.createElement("tr");
      const badge = r.suspended
        ? '<span class="badge err">durduruldu</span>'
        : r.low_balance
          ? '<span class="badge warn">dusuk</span>'
          : '<span class="badge ok">acik</span>';
      tr.innerHTML =
        "<td></td><td class='num'></td><td class='num'></td><td class='num'></td><td></td><td></td>";
      tr.children[0].textContent = r.username;
      tr.children[1].textContent = tl(r.balance_krs);
      tr.children[2].textContent = r.usage_home_secs;
      tr.children[3].textContent = r.usage_fb_secs;
      tr.children[4].innerHTML = badge;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "ghost";
      btn.textContent = "Ac";
      btn.addEventListener("click", () => openDetail(r.username));
      tr.children[5].appendChild(btn);
      tb.appendChild(tr);
    }
    say("");
  } catch (e) {
    say(fmtErr(e), "err");
  }
}

async function openDetail(username) {
  detailUser = username;
  try {
    const v = await invoke("admin_user", { username });
    $("detail").hidden = false;
    $("dname").textContent = v.username;
    $("dmeta").textContent =
      "bakiye " + tl(v.balance_krs) + " · cihaz " + v.devices +
      " · ev-tek " + (v.home_only ? "acik" : "kapali");
    $("detail").scrollIntoView({ block: "nearest" });
  } catch (e) {
    say(fmtErr(e), "err");
  }
}

$("usersbtn").addEventListener("click", loadUsers);

$("topupform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!detailUser) return;
  try {
    const v = await invoke("admin_topup", {
      username: detailUser,
      amountKrs: parseInt($("tp-amt").value, 10),
    });
    $("tp-amt").value = "";
    say(detailUser + " yeni bakiye " + tl(v.balance_krs) + ".", "ok");
    loadUsers();
    openDetail(detailUser);
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("capform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!detailUser) return;
  try {
    const raw = $("cp-cap").value.trim();
    await invoke("admin_limits", {
      username: detailUser,
      dailyHome: null, dailyFb: null, monthlyHome: null, monthlyFb: null,
      fbCap: raw === "" ? null : parseInt(raw, 10),
      homeOnly: null,
    });
    $("cp-cap").value = "";
    say("Tavan yazildi.", "ok");
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("homeonlybtn").addEventListener("click", async () => {
  if (!detailUser) return;
  try {
    const cur = await invoke("admin_user", { username: detailUser });
    await invoke("admin_home_only", { username: detailUser, homeOnly: !cur.home_only });
    say("Sadece-ev " + (!cur.home_only ? "acildi." : "kapatildi."), "ok");
    openDetail(detailUser);
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("hwidbtn").addEventListener("click", async () => {
  if (!detailUser) return;
  try {
    await invoke("admin_hwid_reset", { username: detailUser });
    say("Cihaz slotlari temizlendi.", "ok");
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("suspendbtn").addEventListener("click", async () => {
  if (!detailUser) return;
  try {
    const cur = await invoke("admin_user", { username: detailUser });
    const stop = !cur.suspended;
    await invoke("admin_suspend", { username: detailUser, stop });
    say(stop ? "Hesap durduruldu." : "Hesap acildi.", "ok");
    loadUsers();
    openDetail(detailUser);
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

// ---- davet ----
$("inviteform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  try {
    const v = await invoke("admin_invite", {
      username: $("iv-user").value.trim(),
      openingKrs: parseInt($("iv-bal").value, 10),
      code: $("iv-code").value.trim() === "" ? null : $("iv-code").value.trim(),
    });
    $("codewrap").hidden = false;
    $("codebig").textContent = v.code;
    say("Davet hazir (" + v.username + ").", "ok");
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

// ---- tarifeler ----
async function loadTariffs() {
  try {
    const v = await invoke("admin_tariffs");
    const fb = v.fallback_type === "fixed_krs_per_min"
      ? tl(v.fallback_value) + "/dk sabit"
      : (v.fallback_value / 100).toFixed(0) + "x carpan";
    $("tariffcur").textContent =
      "v" + v.version + " · ev " + tl(v.home_krs_per_min) + "/dk · fallback " + fb +
      " · taban " + v.upstream_min_secs + "sn";
  } catch (e) {
    say(fmtErr(e), "err");
  }
}
$("tariffsbtn").addEventListener("click", loadTariffs);

$("tariffform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  try {
    const fx = $("tf-fixed").value.trim();
    const bp = $("tf-bp").value.trim();
    await invoke("admin_set_tariff", {
      homeKrsPerMin: parseInt($("tf-home").value, 10),
      fallbackFixed: fx === "" ? null : parseInt(fx, 10),
      fallbackBp: bp === "" ? null : parseInt(bp, 10),
    });
    say("Tarife sonraki isteklere uygulanir.", "ok");
    loadTariffs();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

// ---- fallback ----
function paintVendor(v) {
  $("vendorcur").textContent =
    "hat " + v.vendor + " · v" + v.version +
    " · groq taban " + v.groq.upstream_min_secs + "sn" +
    " · openai taban " + v.openai.upstream_min_secs + "sn";
  $("kgroq").className = "dot " + (v.groq.key_set ? "ok" : "idle");
  $("kopenai").className = "dot " + (v.openai.key_set ? "ok" : "idle");
}

async function loadVendor() {
  try {
    const v = await invoke("admin_vendor");
    paintVendor(v);
    const s = await invoke("admin_switch");
    $("swbtn").textContent = "Salter: " + (s.open ? "acik" : "KAPALI");
  } catch (e) {
    say(fmtErr(e), "err");
  }
}
$("vendorbtn").addEventListener("click", loadVendor);

$("swbtn").addEventListener("click", async () => {
  try {
    const s = await invoke("admin_switch");
    await invoke("admin_set_switch", { open: !s.open });
    say(!s.open ? "Fallback acildi." : "Fallback durduruldu (yeni isler bakim retli).", "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("vgroq").addEventListener("click", async () => {
  try {
    await invoke("admin_set_vendor", { vendor: "groq" });
    say("Hat groq.", "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("vopenai").addEventListener("click", async () => {
  try {
    await invoke("admin_set_vendor", { vendor: "openai" });
    say("Hat openai.", "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("vpriceform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  try {
    const fx = $("vp-fixed").value.trim();
    const bp = $("vp-bp").value.trim();
    await invoke("admin_vendor_price", {
      vendor: $("vp-vendor").value,
      fixed: fx === "" ? null : parseInt(fx, 10),
      bp: bp === "" ? null : parseInt(bp, 10),
    });
    say("Hat fiyati yazildi.", "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("vsecbtn").addEventListener("click", async () => {
  try {
    await invoke("admin_vendor_upstream", {
      vendor: $("vp-vendor").value,
      secs: parseInt($("vp-secs").value, 10),
    });
    say("Hat tabani yazildi.", "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

async function keySet(vendor, inputId) {
  try {
    const r = await invoke("admin_key_set", { vendor, key: $(inputId).value });
    $(inputId).value = "";
    say(vendor + " anahtar " + (r.stored ? "girildi." : "temizlendi."), "ok");
    loadVendor();
  } catch (e) {
    say(fmtErr(e), "err");
  }
}
$("keygroq").addEventListener("submit", (ev) => { ev.preventDefault(); keySet("groq", "kg-in"); });
$("keyopenai").addEventListener("submit", (ev) => { ev.preventDefault(); keySet("openai", "ko-in"); });
$("kg-clear").addEventListener("click", async () => {
  try {
    await invoke("admin_key_clear", { vendor: "groq" });
    say("groq anahtar silindi.", "ok");
    loadVendor();
  } catch (e) { say(fmtErr(e), "err"); }
});
$("ko-clear").addEventListener("click", async () => {
  try {
    await invoke("admin_key_clear", { vendor: "openai" });
    say("openai anahtar silindi.", "ok");
    loadVendor();
  } catch (e) { say(fmtErr(e), "err"); }
});

// ---- denetim + diskler ----
$("auditbtn").addEventListener("click", async () => {
  try {
    const v = await invoke("admin_audit");
    const list = $("auditlist");
    list.innerHTML = "";
    const rows = (v.entries || []).slice().reverse();
    if (!rows.length) list.textContent = "kayit yok";
    for (const e of rows.slice(0, 200)) {
      const d = document.createElement("div");
      const t = new Date(e.at * 1000).toLocaleString("tr-TR");
      d.textContent = t + " · " + e.admin + " · " + e.action + " · " + e.detail;
      list.appendChild(d);
    }
  } catch (e) {
    say(fmtErr(e), "err");
  }
});

$("disksbtn").addEventListener("click", async () => {
  try {
    const v = await invoke("admin_disks");
    $("diskscur").textContent = (v.disks || [])
      .map((d) => d.disk + " %" + d.used_pct + (d.warn ? " (UYARI)" : ""))
      .join(" · ") || "—";
  } catch (e) {
    say(fmtErr(e), "err");
  }
});
