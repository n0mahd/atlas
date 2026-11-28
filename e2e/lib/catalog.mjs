// Helpers for the catalog and access journeys: the owner's hand edits in a repo, reading
// what git recorded, the editor, and swapping passkey authenticators.

import fs from "node:fs";
import path from "node:path";
import { expect } from "@playwright/test";
import { git } from "./world.mjs";

/** The hub's read of the store is shared for 3 seconds; changes made by hand show after it. */
export const SNAPSHOT_TTL_MS = 3200;

export const isPhone = (testInfo) => testInfo.project.name === "phone";

/** Writes a file in a repo the way the owner would with an editor (no commit). */
export function handEdit(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

/** Commits files by hand, as the owner working in the repo. */
export function handCommit(world, dir, message, ...files) {
  git(world, dir, "add", ...(files.length ? files : ["-A"]));
  git(world, dir, "commit", "-q", "-m", message);
}

/** The newest commit in `dir`: sha, subject and the files it touched. */
export function lastCommit(world, dir) {
  const sha = git(world, dir, "rev-parse", "HEAD");
  const subject = git(world, dir, "log", "-1", "--format=%s");
  const files = git(world, dir, "show", "--name-only", "--format=", "HEAD").split("\n").filter(Boolean);
  return { sha, subject, files };
}

/** What `file` holds in HEAD. */
export const atHead = (world, dir, rel) => git(world, dir, "show", `HEAD:${rel}`);

/** Opens the editor through the page's pencil, as the owner would. */
export async function openEditor(page, title) {
  await page.getByRole("link", { name: `Edit ${title}` }).click();
  await expect(page.locator("header.masthead h1")).toHaveText(`Edit ${title}`);
  return page.getByLabel("File text");
}

/** Replaces the editor's text and presses Save; waits for the next page to load. */
export async function saveText(page, text) {
  await page.getByLabel("File text").fill(text);
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load" }),
    page.getByRole("button", { name: "Save", exact: true }).click(),
  ]);
}

/**
 * A second device (such as a security key) for adding a passkey while signed in. Since
 * decision 0038 the owner first confirms with an enrolled passkey, then registers the new
 * one: the laptop's authenticator stays for the confirmation, and the moment the browser
 * asks to start the enrolment it is set aside and the new device plugged in, as a person
 * taps the laptop and then the key. Chrome won't pick between two virtual authenticators
 * by itself, so they are never present together. Requests pass through untouched.
 * `restore()` puts the laptop's authenticator back with its credentials.
 */
export async function swapAuthenticator(page, auth) {
  const options = {
    protocol: "ctap2",
    transport: "usb",
    hasResidentKey: true,
    hasUserVerification: true,
    isUserVerified: true,
    automaticPresenceSimulation: true,
  };
  let saved = [];
  let second = null;
  await page.route("**/auth/enrol/start", async (route) => {
    if (!second) {
      saved = await auth.credentials();
      await auth.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: auth.authenticatorId });
      second = (await auth.cdp.send("WebAuthn.addVirtualAuthenticator", { options })).authenticatorId;
    }
    await route.continue();
  });
  return {
    credentials: async () =>
      second ? (await auth.cdp.send("WebAuthn.getCredentials", { authenticatorId: second })).credentials : [],
    async restore() {
      await page.unroute("**/auth/enrol/start");
      if (!second) return;
      await auth.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: second });
      const back = await auth.cdp.send("WebAuthn.addVirtualAuthenticator", {
        options: { ...options, transport: "internal" },
      });
      for (const credential of saved)
        await auth.cdp.send("WebAuthn.addCredential", { authenticatorId: back.authenticatorId, credential });
      auth.authenticatorId = back.authenticatorId;
      second = null;
    },
  };
}
