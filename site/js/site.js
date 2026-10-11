// lintel.thisissbk.com: the menu, things arriving once, the gallery, and the island in the hero.
// No library, no network call, nothing stored.
(() => {
  "use strict";
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // The menu on a narrow screen.
  const nav = $(".nav");
  const toggle = $(".nav-toggle");
  if (toggle) {
    const shut = () => { nav.classList.remove("open"); toggle.setAttribute("aria-expanded", "false"); };
    toggle.addEventListener("click", () => toggle.setAttribute("aria-expanded", String(nav.classList.toggle("open"))));
    addEventListener("keydown", (e) => { if (e.key === "Escape" && nav.classList.contains("open")) { shut(); toggle.focus(); } });
    matchMedia("(min-width: 861px)").addEventListener("change", shut);
  }

  // Sections settle in as they arrive, once.
  const reveals = $$(".reveal");
  if (!("IntersectionObserver" in window) || still) reveals.forEach((el) => el.classList.add("in"));
  else {
    const seen = new IntersectionObserver((entries) => {
      for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); seen.unobserve(e.target); }
    }, { rootMargin: "0px 0px -8% 0px" });
    reveals.forEach((el) => seen.observe(el));
  }

  // A picture whose file is not there leaves its frame, with the mascot's mark in it.
  for (const img of $$(".shot img")) {
    const gone = () => img.closest(".shot").classList.add("empty");
    if (img.complete && !img.naturalWidth) gone(); else img.addEventListener("error", gone);
  }

  // The gallery opens a picture large; the arrow keys go to the next one.
  const box = $("dialog.lightbox");
  if (box) {
    const shots = $$(".gallery .shot");
    const big = $("img", box);
    const cap = $("p", box);
    let at = 0;
    const show = (i) => {
      const live = shots.filter((s) => !s.classList.contains("empty"));
      if (!live.length) return;
      at = (i + live.length) % live.length;
      const img = $("img", live[at]);
      big.src = img.currentSrc || img.src; big.alt = img.alt;
      cap.textContent = $("figcaption", live[at].parentNode).textContent;
      if (!box.open) box.showModal();
    };
    shots.forEach((s) => s.addEventListener("click", () => show(shots.filter((x) => !x.classList.contains("empty")).indexOf(s))));
    box.addEventListener("click", (e) => { if (e.target === box) box.close(); });
    box.addEventListener("keydown", (e) => { if (e.key === "ArrowRight") show(at + 1); if (e.key === "ArrowLeft") show(at - 1); });
  }

  // ── The island ──────────────────────────────────────────────────────────────
  const isl = $(".island");
  if (!isl) return;

  // The mascot, redrawn from the app's engine (src/mascot/engine.ts): an orb in the accent colour,
  // one dark band across its upper half, one bar of light in the band. It bobs, blinks and dances.
  const TAU = Math.PI * 2;
  const BODY = [142, 162, 255], WHITE = [255, 255, 255], BLACK = [0, 0, 0];
  const TONE = { ask: [245, 165, 36], done: [52, 211, 153] };
  const mix = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);
  const rgba = (c, a = 1) => `rgba(${c.map(Math.round).join(",")},${a})`;
  const cv = $("canvas", isl);
  const ctx = cv.getContext("2d");
  const BOX = 30, D = 21;
  const scale = Math.min(devicePixelRatio || 1, 2) * 1.5;
  cv.width = cv.height = Math.round(BOX * scale);
  const bot = { face: "idle", w: { idle: 1, ask: 0, done: 0 }, t: 0, open: 1, blinkT: 0, blinkIn: 2.4, dancing: false, dance: 0 };

  function drawBot() {
    const { w, t } = bot;
    const d = D, r = d * 0.46, th = Math.max(1.5, d * 0.074), u = d / 58;
    const beat = (t * 112) / 60, hop = Math.abs(Math.sin(Math.PI * beat)), k = bot.dance;
    ctx.setTransform(scale, 0, 0, scale, 0, 0);
    ctx.clearRect(0, 0, BOX, BOX);
    ctx.save();
    ctx.translate(BOX / 2 + 0.08 * r * Math.sin(Math.PI * beat) * k, BOX / 2 + 1 + (still ? 0 : Math.sin((t * TAU) / 2.8) * 1.5 * u) - 0.2 * r * hop * k);
    ctx.rotate(0.1 * Math.sin(Math.PI * beat) * k);
    const shade = ctx.createLinearGradient(0, -r, 0, r);
    shade.addColorStop(0, rgba(mix(BODY, WHITE, 0.3)));
    shade.addColorStop(0.55, rgba(BODY));
    shade.addColorStop(1, rgba(mix(BODY, BLACK, 0.32)));
    ctx.fillStyle = shade;
    ctx.beginPath(); ctx.arc(0, 0, r, 0, TAU); ctx.fill();
    ctx.fillStyle = "rgba(7,7,8,.94)";
    ctx.beginPath(); ctx.roundRect(-d * 0.35, -d * 0.24, d * 0.7, d * 0.28, d * 0.14); ctx.fill();
    ctx.translate(0, -d * 0.1);
    const tint = mix(mix(WHITE, TONE.ask, w.ask), TONE.done, w.done);
    ctx.fillStyle = ctx.strokeStyle = rgba(tint);
    ctx.shadowColor = rgba(tint, 0.75);
    ctx.shadowBlur = Math.max(2, d * 0.12) * scale;
    ctx.lineWidth = th; ctx.lineCap = "round";
    const bar = (wd, ht) => { ht = Math.max(ht, 0.7); ctx.beginPath(); ctx.roundRect(-wd / 2, -ht / 2, wd, ht, ht / 2); ctx.fill(); };
    if (w.idle > 0.02) { ctx.globalAlpha = w.idle; bar(d * 0.3 * (1 + 0.25 * hop * k), th * bot.open); }
    if (w.ask > 0.02) { ctx.globalAlpha = w.ask * (still ? 1 : 0.5 + 0.5 * (0.5 + 0.5 * Math.sin((t * TAU) / 1.2))); bar(d * 0.46, th * 1.3); }
    if (w.done > 0.02) {
      ctx.globalAlpha = w.done;
      ctx.beginPath(); ctx.moveTo(-d * 0.13, d * 0.04); ctx.quadraticCurveTo(0, -d * 0.12, d * 0.13, d * 0.04); ctx.stroke();
    }
    ctx.restore();
  }
  function stepBot(dt) {
    const k = still ? 1 : 1 - Math.exp(-dt * 11);
    for (const n in bot.w) bot.w[n] += ((n === bot.face ? 1 : 0) - bot.w[n]) * k;
    bot.t += dt;
    if ((bot.blinkIn -= dt) < 0) { bot.blinkIn = 3 + Math.random() * 3; bot.blinkT = 0.16; }
    bot.blinkT -= dt;
    bot.open += ((bot.blinkT > 0 ? 0.1 : 1) - bot.open) * (1 - Math.exp(-dt * 50));
    bot.dance += Math.sign((bot.dancing ? 1 : 0) - bot.dance) * Math.min(Math.abs((bot.dancing ? 1 : 0) - bot.dance), dt / 0.4);
  }

  const text = $("[data-peek]", isl);
  const num = $("[data-num]", isl);
  const level = $(".isl-level i", isl);
  const allow = $(".isl-btn.go", isl);
  const playBtn = $("[data-playpause]", isl);
  const picks = $$("[data-pick]");
  const caps = $$("[data-cap]");
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  let run = 0, answer = null, manual = null;

  function scene(name) {
    isl.dataset.scene = name;
    picks.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.pick === name)));
    caps.forEach((c) => (c.hidden = c.dataset.cap !== name));
  }
  const mode = (m) => (isl.dataset.mode = m);
  const playing = (on) => {
    isl.dataset.play = on ? "1" : "0"; bot.dancing = on && !still;
    playBtn.setAttribute("aria-label", on ? "Pause" : "Play");
    $("path", playBtn).setAttribute("d", on ? "M8.5 5.5v13M15.5 5.5v13" : "M8 5.5v13l11-6.5z");
  };

  async function show(id, name, hold) {
    const step = async (ms) => { await sleep(ms); if (id !== run) throw 0; };
    mode("hidden"); await step(450);
    scene(name); playing(false); bot.face = "idle";
    if (name === "ask") {
      text.textContent = "Claude Code asks"; bot.face = "ask";
      await step(350); mode("peek"); await step(1300); mode("open");
      const said = await new Promise((res) => {
        answer = res;
        if (!hold) sleep(3800).then(() => { if (id !== run) return; allow.classList.add("press"); sleep(200).then(() => { allow.classList.remove("press"); res("Allowed"); }); });
      });
      answer = null; if (id !== run) throw 0;
      bot.face = "done"; text.textContent = said; mode("peek"); await step(1700);
    } else if (name === "level") {
      text.textContent = "Volume"; num.textContent = "40"; level.style.transform = "scaleX(.4)";
      await step(350); mode("peek"); await step(500);
      level.style.transform = "scaleX(.62)";
      for (let v = 42; v <= 62; v += 2) { num.textContent = v; await step(28); }
      await step(2100);
    } else {
      text.textContent = "Night Ferry"; playing(true);
      await step(350); mode("peek"); await step(1500); mode("open");
      if (hold) await new Promise(() => {});
      await step(5200); mode("peek"); await step(1200);
    }
    mode("hidden"); await step(500); bot.face = "idle";
  }
  async function loop() {
    const id = ++run;
    try { for (;;) for (const name of manual ? [manual] : ["ask", "level", "play"]) await show(id, name, !!manual); } catch { /* another run took over */ }
  }

  if (still) {
    // One still picture: the card open, waiting. The buttons under the stage change the picture.
    const pose = (name) => {
      scene(name); bot.face = name === "ask" ? "ask" : "idle";
      text.textContent = name === "level" ? "Volume" : name === "ask" ? "Claude Code asks" : "Night Ferry";
      if (name === "level") { num.textContent = "62"; level.style.transform = "scaleX(.62)"; }
      mode(name === "level" ? "peek" : "open"); stepBot(1); drawBot();
    };
    pose("ask");
    picks.forEach((b) => b.addEventListener("click", () => pose(b.dataset.pick)));
    $$(".isl-ask .isl-btn", isl).forEach((b) => b.addEventListener("click", () => { bot.face = "done"; text.textContent = b.dataset.say; mode("peek"); stepBot(1); drawBot(); }));
    return;
  }

  picks.forEach((b) => b.addEventListener("click", () => { manual = b.dataset.pick; loop(); }));
  $$(".isl-ask .isl-btn", isl).forEach((b) => b.addEventListener("click", () => answer && answer(b.dataset.say)));
  playBtn.addEventListener("click", () => playing(isl.dataset.play !== "1"));

  // The island runs only while it is on screen: off screen it draws nothing and no timer of its is left to matter.
  let raf = 0, last = 0;
  const frame = (now) => { stepBot(Math.min(0.05, (now - last) / 1000)); last = now; drawBot(); raf = requestAnimationFrame(frame); };
  // A way to stop the moving picture, and to start it again.
  const pause = $("[data-pause]");
  let paused = false, onScreen = false;
  const go = () => { cancelAnimationFrame(raf); if (onScreen && !paused) { last = performance.now(); raf = requestAnimationFrame(frame); loop(); } else run++; };
  pause.addEventListener("click", () => { paused = !paused; pause.textContent = paused ? "Play" : "Pause"; pause.setAttribute("aria-pressed", String(paused)); go(); });
  picks.forEach((b) => b.addEventListener("click", () => { if (paused) pause.click(); }));
  new IntersectionObserver(([e]) => {
    onScreen = e.isIntersecting; if (paused) return;
    cancelAnimationFrame(raf);
    if (e.isIntersecting) { last = performance.now(); raf = requestAnimationFrame(frame); loop(); }
    else { run++; mode("hidden"); }
  }, { threshold: 0.15 }).observe(isl.parentNode);
})();

// The download card shows the latest release, asked from GitHub. If that fails, what the page was built with stays,
// and the button's own address already points at the latest installer.
(() => {
  const slots = document.querySelectorAll("[data-latest]");
  if (!slots.length) return;
  fetch("https://api.github.com/repos/shakibbinkabir/lintel/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((release) => {
      const asset = (release.assets || []).find((a) => /^lintel-\d[\d.]*-setup\.exe$/.test(a.name));
      if (!asset) return;
      const text = {
        version: release.tag_name.replace(/^v/, ""),
        size: (asset.size / 1048576).toFixed(1) + " MB",
        date: new Date(release.published_at).toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" }),
        file: asset.name,
      };
      slots.forEach((el) => {
        const key = el.dataset.latest;
        if (key === "installer") el.href = asset.browser_download_url;
        else if (text[key]) el.textContent = text[key];
      });
    })
    .catch(() => {});
})();
