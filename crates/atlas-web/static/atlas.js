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
    if (!toggle) return;
    var collapsed = root.getAttribute("data-rail") === "collapsed";
    if (collapsed) root.removeAttribute("data-rail"); else root.setAttribute("data-rail", "collapsed");
    store(collapsed ? "open" : "collapsed");
    toggle.setAttribute("aria-expanded", collapsed ? "true" : "false");
  });

  // Pickers marked data-autosubmit submit their form when changed.
  document.addEventListener("change", function (event) {
    var el = event.target;
    if (el.matches && el.matches("select[data-autosubmit]") && el.form) el.form.requestSubmit();
  });

  document.addEventListener("DOMContentLoaded", function () {
    var toggle = document.querySelector("[data-rail-toggle]");
    if (toggle) toggle.setAttribute("aria-expanded", root.getAttribute("data-rail") === "collapsed" ? "false" : "true");
  });
})();
