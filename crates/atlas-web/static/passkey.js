// Passkey ceremonies for Atlas: enrol and sign in. Talks JSON to /auth/*; webauthn-rs
// encodes binary fields as base64url strings, browsers want ArrayBuffers.
(function () {
  function toBuf(b64) {
    var s = b64.replace(/-/g, "+").replace(/_/g, "/");
    while (s.length % 4) s += "=";
    var bin = atob(s), out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out.buffer;
  }
  function toB64(buf) {
    var bytes = new Uint8Array(buf), s = "";
    for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function csrf() {
    var m = document.querySelector('meta[name="atlas-csrf"]');
    return m ? m.content : "";
  }
  async function post(url, body) {
    var res = await fetch(url, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Atlas-CSRF": csrf() },
      body: JSON.stringify(body)
    });
    if (!res.ok) throw new Error((await res.text()).trim() || "Something went wrong.");
    return res.json();
  }

  async function enrol(box) {
    var code = box.querySelector('[name="code"]');
    var name = box.querySelector('[name="name"]');
    var start = await post("/auth/enrol/start", { code: code ? code.value : "", name: name ? name.value : "" });
    var pk = start.options.publicKey;
    pk.challenge = toBuf(pk.challenge);
    pk.user.id = toBuf(pk.user.id);
    (pk.excludeCredentials || []).forEach(function (c) { c.id = toBuf(c.id); });
    var cred = await navigator.credentials.create({ publicKey: pk });
    var done = await post("/auth/enrol/finish", {
      flow: start.flow,
      credential: {
        id: cred.id,
        rawId: toB64(cred.rawId),
        type: cred.type,
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
        response: {
          attestationObject: toB64(cred.response.attestationObject),
          clientDataJSON: toB64(cred.response.clientDataJSON),
          transports: cred.response.getTransports ? cred.response.getTransports() : undefined
        }
      }
    });
    location.assign(done.next);
  }

  async function signIn(box) {
    var start = await post("/auth/signin/start", { next: box.dataset.next || "/" });
    var pk = start.options.publicKey;
    pk.challenge = toBuf(pk.challenge);
    (pk.allowCredentials || []).forEach(function (c) { c.id = toBuf(c.id); });
    var cred = await navigator.credentials.get({ publicKey: pk });
    var done = await post("/auth/signin/finish", {
      flow: start.flow,
      credential: {
        id: cred.id,
        rawId: toB64(cred.rawId),
        type: cred.type,
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
        response: {
          authenticatorData: toB64(cred.response.authenticatorData),
          clientDataJSON: toB64(cred.response.clientDataJSON),
          signature: toB64(cred.response.signature),
          userHandle: cred.response.userHandle ? toB64(cred.response.userHandle) : null
        }
      }
    });
    location.assign(done.next);
  }

  document.addEventListener("click", function (event) {
    var button = event.target.closest("[data-passkey] [data-action]");
    if (!button) return;
    var box = button.closest("[data-passkey]");
    var error = box.querySelector("[data-error]");
    if (!window.PublicKeyCredential) {
      error.textContent = "This browser can't use passkeys.";
      error.hidden = false;
      return;
    }
    error.hidden = true;
    button.disabled = true;
    var run = button.dataset.action === "enrol" ? enrol : signIn;
    run(box).catch(function (e) {
      error.textContent = e && e.name === "NotAllowedError" ? "The passkey prompt was closed. Try again." : (e.message || String(e));
      error.hidden = false;
    }).finally(function () { button.disabled = false; });
  });
})();
