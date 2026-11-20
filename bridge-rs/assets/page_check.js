/* The bridge's layout check of an agent's page (page_check.rs): put first in the page's head, it notes the page's
   JS errors, then once the page has drawn itself measures what a person would see broken at this width and replaces
   the document with its findings (JSON in <pre id="bip-check">), read from Chromium's --dump-dom. Never shipped to a
   person: only the copy the checker opens carries it. */
(function () {
  var errors = [];
  addEventListener("error", function (e) { errors.push(String(e.message || e.type).slice(0, 120)); });
  addEventListener("unhandledrejection", function (e) { errors.push(String(e.reason && e.reason.message || e.reason).slice(0, 120)); });
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
    var texts = [];
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
      if (own && s.position !== "absolute" && s.position !== "fixed" && texts.length < 400) texts.push([el, r]);
      if (/\b(card|kpi|tab)\b/.test(el.className) || /^(SECTION|TABLE|UL|OL)$/.test(el.tagName)) {
        if (!words(el) && !el.querySelector("img,svg,canvas,input,select,textarea,button,video,audio,iframe")) empty.push(name(el));
      }
    }
    for (var a = 0; a < texts.length && overlap.length < 5; a++) for (var b = a + 1; b < texts.length; b++) {
      var x = texts[a], y = texts[b];
      if (x[0].contains(y[0]) || y[0].contains(x[0])) continue;
      var w = Math.min(x[1].right, y[1].right) - Math.max(x[1].left, y[1].left), h = Math.min(x[1].bottom, y[1].bottom) - Math.max(x[1].top, y[1].top);
      if (w > 4 && h > 4) { overlap.push(name(x[0]) + " / " + name(y[0])); break; }
    }
    var out = {
      width: width, page: document.documentElement.scrollWidth, wide: wide.slice(0, 5), cut: cut.slice(0, 5),
      overlap: overlap, empty: empty.slice(0, 5), errors: errors.slice(0, 5), text: words(document.body || document.documentElement).length,
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
