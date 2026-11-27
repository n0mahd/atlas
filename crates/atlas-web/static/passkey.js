// Passkey ceremonies for Atlas: enrol, sign in, and send changes to the machines. Talks
// JSON to /auth/* and /send/*; webauthn-rs encodes binary fields as base64url strings,
// browsers want ArrayBuffers.
(function () {
  if (window.atlasPasskey) return;
  window.atlasPasskey = true;
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
    var token = box.querySelector('[name="token"]');
    var name = box.querySelector('[name="name"]');
    var start = await post("/auth/enrol/start", { token: token ? token.value : "", name: name ? name.value : "" });
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

  // Sends saved changes to the machines: the passkey signs the update's digest
  // (decisions 0019, 0022). Resolves to where to go next; 'nothing' needs no tap.
  async function send(next) {
    var opts = await post("/send/start", { next: next });
    if (opts.nothing) return { nothing: true, next: opts.next, note: opts.note };
    var cred = await navigator.credentials.get({ publicKey: {
      challenge: toBuf(opts.challenge),
      rpId: opts.rpId,
      allowCredentials: opts.allowCredentials.map(function (c) { return { id: toBuf(c.id), type: c.type }; }),
      userVerification: opts.userVerification,
      timeout: opts.timeout
    } });
    return post("/send/finish", {
      next: next,
      credential: {
        credential_id: toB64(cred.rawId),
        authenticator_data: toB64(cred.response.authenticatorData),
        client_data_json: toB64(cred.response.clientDataJSON),
        signature: toB64(cred.response.signature)
      }
    });
  }

  function withParam(url, key) {
    var u = new URL(url, location.href);
    u.searchParams.delete("notsent");
    u.searchParams.set(key, "1");
    return u.pathname + u.search + u.hash;
  }

  // A Send button: its box names where to go, else this page marked as sent.
  async function sendButton(box) {
    var done = await send(box.dataset.next || withParam(location.href, "sent"));
    if (done.nothing && done.note) throw new Error(done.note);
    location.assign(done.next);
  }

  // Save sends: post the form, then send, with one passkey tap. Any failure of the save
  // itself falls back to a normal submit, which shows the error page.
  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form.matches("form[data-send-on-save]") || form.dataset.plain || !window.PublicKeyCredential || !window.fetch) return;
    event.preventDefault();
    var button = form.querySelector('[type="submit"]');
    if (button) button.disabled = true;
    var plain = function () { form.dataset.plain = "1"; form.submit(); };
    fetch(form.action, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Accept": "application/json" },
      body: new URLSearchParams(new FormData(form))
    }).then(function (res) {
      if (!res.ok || !(res.headers.get("content-type") || "").includes("application/json")) return plain();
      return res.json().then(function (saved) {
        if (!saved.send) return location.assign(saved.next);
        return send(withParam(saved.next, "sent"))
          .then(function (done) { location.assign(done.next); })
          .catch(function () { location.assign(withParam(saved.next, "notsent")); });
      });
    }, plain);
  });

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
    var action = button.dataset.action;
    var run = action === "enrol" ? enrol : action === "send" ? sendButton : signIn;
    run(box).catch(function (e) {
      error.textContent = e && e.name === "NotAllowedError" ? "The passkey prompt was closed. Try again." : (e.message || String(e));
      error.hidden = false;
    }).finally(function () { button.disabled = false; });
  });
})();
