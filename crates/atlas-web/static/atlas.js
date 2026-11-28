// Atlas page behaviour. Loaded from /static so the CSP needs no inline script.
(function () {
  var root = document.documentElement;
  var KEY = "atlas.rail";

  function stored() {
    try { return localStorage.getItem(KEY); } catch (e) { return null; }
  }
  function store(value) {
    try { localStorage.setItem(KEY, value); } catch (e) { /* private mode: not remembered */ }
  }

  // Apply the remembered rail state before first paint (this file loads in <head>).
  if (stored() === "collapsed") root.setAttribute("data-rail", "collapsed");

  document.addEventListener("click", function (event) {
    var toggle = event.target.closest("[data-rail-toggle]");
    if (toggle) {
      var collapsed = root.getAttribute("data-rail") === "collapsed";
      if (collapsed) root.removeAttribute("data-rail"); else root.setAttribute("data-rail", "collapsed");
      store(collapsed ? "open" : "collapsed");
      toggle.setAttribute("aria-expanded", collapsed ? "true" : "false");
      return;
    }
    var copy = event.target.closest("[data-copy]");
    if (copy && navigator.clipboard && navigator.clipboard.writeText) {
      var text = copy.getAttribute("data-copy");
      var original = copy.textContent;
      navigator.clipboard.writeText(text).then(function () {
        copy.textContent = "Copied";
        window.setTimeout(function () { copy.textContent = original; }, 1500);
      });
    }
  });

  // Pickers marked data-autosubmit submit their form when changed.
  document.addEventListener("change", function (event) {
    var el = event.target;
    if (el.matches && el.matches("select[data-autosubmit]") && el.form) el.form.requestSubmit();
  });

  // Slow forms marked data-busy disable their button while the hub works, so a second
  // press doesn't start the same job twice.
  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form.hasAttribute || !form.hasAttribute("data-busy")) return;
    var button = form.querySelector("button[type=submit]");
    if (!button) return;
    button.disabled = true;
    button.textContent = form.getAttribute("data-busy");
  });

  document.addEventListener("DOMContentLoaded", function () {
    var toggle = document.querySelector("[data-rail-toggle]");
    if (toggle) toggle.setAttribute("aria-expanded", root.getAttribute("data-rail") === "collapsed" ? "false" : "true");
  });
})();
