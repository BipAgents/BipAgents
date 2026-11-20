/* The bridge's layout check of an agent's page (page_check.rs): put first in the page's head, it notes the page's
   JS errors, then once the page has drawn itself measures what a person would see broken at this width and replaces
   the document with its findings (JSON in <pre id="bip-check">), read from Chromium's --dump-dom. Never shipped to a
   person: only the copy the checker opens carries it. */
(function () {
  var errors = [];
  addEventListener("error", function (e) { errors.push(String(e.message || e.type).slice(0, 120)); });
  addEventListener("unhandledrejection", function (e) { errors.push(String(e.reason && e.reason.message || e.reason).slice(0, 120)); });
  // the store's writes made when the page opened (refused: only what the person does is written, store.js)
  var unsaved = 0, warn = console.warn;
  console.warn = function (m) { if (/^Bip\.store: not saved/.test(String(m))) unsaved++; return warn.apply(console, arguments); };
  function words(el) { return (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim(); }
  function name(el) {
    var cls = typeof el.className === "string" && el.className.trim() ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".") : "";
    var text = words(el);
    return "<" + el.tagName.toLowerCase() + cls + ">" + (text ? " « " + (text.length > 48 ? text.slice(0, 47) + "…" : text) + " »" : "");
  }
  function scrolls(el) {
    // inside a box meant to scroll sideways (a .table-wrap, a wide diagram): nothing is lost there
    for (var p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      var o = getComputedStyle(p).overflowX;
      if (o === "auto" || o === "scroll") return true;
    }
    return false;
  }
  function seen(el, s) {
    if (s.display === "none" || s.visibility === "hidden" || el.closest("[hidden],[aria-hidden=true],svg,.map-live")) return false;
    var r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  }
  function check() {
    var width = innerWidth, wide = [], cut = [], overlap = [], empty = [], all = document.body ? document.body.querySelectorAll("*") : [];
    var texts = [], stored = !!(window.Bip && Bip.store);
    for (var i = 0; i < all.length; i++) {
      var el = all[i], s = getComputedStyle(el);
      if (/^(SCRIPT|STYLE|TEMPLATE|BR)$/.test(el.tagName) || !seen(el, s)) continue;
      var r = el.getBoundingClientRect();
      if ((r.right > width + 1 || r.left < -1) && !scrolls(el) && s.position !== "fixed") {
        var up = el.parentElement, pr = up && up.getBoundingClientRect();
        // only the outermost: its children overflow with it
        if (!pr || up === document.body || !(pr.right > width + 1 || pr.left < -1)) wide.push(name(el) + " " + Math.round(r.width) + " px");
      }
      var own = false;
      for (var c = el.firstChild; c; c = c.nextSibling) if (c.nodeType === 3 && c.nodeValue.trim()) { own = true; break; }
      var hides = /hidden|clip/.test(s.overflowX) || s.textOverflow === "ellipsis";
      if (own && hides && el.scrollWidth > el.clientWidth + 2 && el.clientWidth > 0) cut.push(name(el));
      if (own && /hidden|clip/.test(s.overflowY) && el.scrollHeight > el.clientHeight * 1.3 + 4) cut.push(name(el));
      // its line boxes (a word run over two lines is two boxes, not the block they span)
      if (own && s.position !== "absolute" && s.position !== "fixed" && texts.length < 400) texts.push([el, [].slice.call(el.getClientRects())]);
      // a page that keeps its data in its store draws it once the store answers (none here): not empty for that
      if (!stored && (/\b(card|kpi|tab)\b/.test(el.className) || /^(SECTION|TABLE|UL|OL)$/.test(el.tagName))) {
        if (!words(el) && !el.querySelector("img,svg,canvas,input,select,textarea,button,video,audio,iframe")) empty.push(name(el));
      }
    }
    for (var a = 0; a < texts.length && overlap.length < 5; a++) for (var b = a + 1; b < texts.length; b++) {
      var x = texts[a], y = texts[b];
      if (x[0].contains(y[0]) || y[0].contains(x[0])) continue;
      var hit = x[1].some(function (p) { return y[1].some(function (q) {
        return Math.min(p.right, q.right) - Math.max(p.left, q.left) > 4 && Math.min(p.bottom, q.bottom) - Math.max(p.top, q.top) > 4;
      }); });
      if (hit) { overlap.push(name(x[0]) + " / " + name(y[0])); break; }
    }
    // a number broken over two lines (« 12 345,6 » / « 7 € »: two numbers to the eye), and markup shown as words
    var split = [], markup = [], walk = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT), t, seenNodes = 0;
    while ((t = walk.nextNode()) && seenNodes < 600) {
      var up2 = t.parentElement;
      if (!up2 || /^(SCRIPT|STYLE|TEXTAREA|PRE|CODE)$/.test(up2.tagName) || up2.closest("svg") || !t.data.trim()) continue;
      seenNodes++;
      if (/<\/?[a-z][a-z0-9]*(\s+[a-z-]+=|\s*\/?>)/i.test(t.data) && markup.length < 3) markup.push(name(up2));
      var re = /\d[\d\u00a0\u202f\u2009 ,.]*\d/g, m2;
      while ((m2 = re.exec(t.data)) && split.length < 3) {
        var top = null, broken = false;
        for (var k = m2.index; k < m2.index + m2[0].length; k++) {
          if (!/\d/.test(t.data[k])) continue;
          var rg = document.createRange(); rg.setStart(t, k); rg.setEnd(t, k + 1);
          var b2 = rg.getBoundingClientRect();
          if (!b2.width) continue;
          if (top !== null && Math.abs(b2.top - top) > 4) { broken = true; break; }
          top = b2.top;
        }
        if (broken) split.push(name(up2));
      }
    }
    // the first button pressed once: what breaks at the first tap (a script error) is heard too
    var first = document.querySelector("button:not([disabled])");
    if (first) try { first.click(); } catch (e) { errors.push(String(e && e.message || e).slice(0, 120)); }
    var out = {
      width: width, page: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight, wide: wide.slice(0, 5), cut: cut.slice(0, 5),
      overlap: overlap, empty: empty.slice(0, 5), errors: errors.slice(0, 5), text: words(document.body || document.documentElement).length,
      split: split, markup: markup, unsaved: unsaved,
    };
    // the checker's frame (page_check.rs) hears it; alone, the page shows it
    if (window.parent !== window) return window.parent.postMessage(out, "*");
    var pre = document.createElement("pre");
    pre.id = "bip-check";
    pre.textContent = JSON.stringify(out);
    document.documentElement.replaceChildren(pre);
  }
  // called by the line the checker puts last in the page: after the kit and the page drew themselves
  window.__bipCheck = check;
})();
