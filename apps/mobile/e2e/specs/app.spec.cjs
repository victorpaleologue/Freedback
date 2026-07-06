// Native UX-path suite: drives the REAL Freedback app (webview + Tauri IPC +
// Rust core) through WebDriver, against a REAL local feedback server.
// One session, flows in user order: first run → look things up → contribute →
// manage "My feedback" → manage "My key" → settings.
"use strict";

const { SERVER_URL } = require("../harness.cjs");

const GS1_NUTELLA = "https://id.gs1.org/01/03017620422003";
const GS1_BOOK = "https://id.gs1.org/01/09780306406157";

async function waitView(name) {
  await $(`#view-${name}`).waitForDisplayed();
}

async function goHome() {
  const back = await $("#nav-back");
  if (await back.isDisplayed()) await back.click();
  await waitView("home");
}

async function lookUp(text) {
  await goHome();
  const input = await $("#resolve-input");
  await input.setValue(text);
  await $("#resolve-form button[type=submit]").click();
}

async function openJournal() {
  await goHome();
  await $("#nav-journal").click();
  await waitView("journal");
}

async function publishOk() {
  await $("#fb-ok").waitForDisplayed();
}

let exportedPem = null;

describe("first run", () => {
  it("boots to the Home view", async () => {
    await waitView("home");
    await expect($("#resolve-input")).toBeDisplayed();
  });

  it("has an empty journal", async () => {
    await openJournal();
    await expect($("#journal-empty")).toBeDisplayed();
  });

  it("keeps Scan disabled on desktop (tauri-plugin-barcode-scanner is mobile-only)", async () => {
    await goHome();
    await expect($("#scan-btn")).not.toBeEnabled();
  });

  it("mints the account key on first export (the key IS the account)", async () => {
    await goHome();
    await $("#nav-key").click();
    await waitView("key");
    await $("#key-export-btn").click();
    await browser.waitUntil(async () =>
      (await $("#key-export").getValue()).includes("BEGIN PRIVATE KEY")
    );
    exportedPem = await $("#key-export").getValue();
  });

  it("also shows the key as a scannable QR code", async () => {
    await expect($("#key-qr")).toBeDisplayed();
    await expect($("#key-qr svg")).toBeExisting();
  });

  it("keeps Scan QR disabled on desktop (same mobile-only plugin)", async () => {
    await expect($("#key-scan-btn")).not.toBeEnabled();
  });
});

describe("resolving input", () => {
  it("resolves an EAN-13 barcode to its GS1 Digital Link", async () => {
    await lookUp("3017620422003");
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText(GS1_NUTELLA);
  });

  it("resolves an ISBN-13 (hyphenated) to the same GS1 namespace", async () => {
    await lookUp("978-0-306-40615-7");
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText(GS1_BOOK);
  });

  it("converts an ISBN-10 to its ISBN-13 EAN", async () => {
    await lookUp("0306406152");
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText(GS1_BOOK);
  });

  it("passes an https URL through unchanged", async () => {
    await lookUp("https://example.com/item/1");
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText("https://example.com/item/1");
  });

  it("extracts a GTIN embedded in shared free text", async () => {
    await lookUp("Just scanned this: 3017620422003 — thoughts?");
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText(GS1_NUTELLA);
  });

  it("rejects a mistyped check digit with a clear error", async () => {
    await lookUp("3017620422004");
    const error = await $("#resolve-error");
    await error.waitForDisplayed();
    await expect(error).toHaveText(expect.stringContaining("check digit"));
    // Still on Home — nothing resolved.
    await expect($("#view-home")).toBeDisplayed();
  });

  it("rejects garbage with a typed error", async () => {
    await lookUp("hello world");
    const error = await $("#resolve-error");
    await error.waitForDisplayed();
    await expect(error).toHaveText(expect.stringContaining("recognized"));
  });

  it("an unreviewed target shows an empty (not broken) feedback screen", async () => {
    await lookUp("https://example.com/never-reviewed");
    await waitView("feedback");
    await expect($("#fb-star-avg")).toHaveText("–");
    await expect($("#fb-star-count")).toHaveText(expect.stringContaining("no ratings"));
  });
});

describe("contributing feedback", () => {
  before(async () => {
    await lookUp("3017620422003");
    await waitView("feedback");
  });

  it("publishes a star rating and the aggregate updates", async () => {
    await $("#c-stars").selectByVisibleText("4");
    await $("#c-stars-send").click();
    await publishOk();
    await browser.waitUntil(async () => (await $("#fb-star-avg").getText()) === "★ 4.0");
    await expect($("#fb-star-count")).toHaveText(expect.stringContaining("1 rating"));
  });

  it("publishes a comment and it appears in the list", async () => {
    await $("#c-comment").setValue("great hazelnut ratio");
    await $("#c-comment-send").click();
    await publishOk();
    await browser.waitUntil(async () =>
      (await $("#fb-comments").getText()).includes("great hazelnut ratio")
    );
  });

  it("publishes a tag and it appears in the list", async () => {
    await $("#c-tag").setValue("breakfast");
    await $("#c-tag-send").click();
    await publishOk();
    await browser.waitUntil(async () =>
      (await $("#fb-tags").getText()).includes("breakfast")
    );
  });

  it("publishes a thumb and the tally updates", async () => {
    await $("#c-thumb-up").click();
    await publishOk();
    await browser.waitUntil(async () => (await $("#fb-thumbs-up").getText()) === "👍 1");
  });

  it("publishes an issue report and it appears in its own list", async () => {
    await $("#c-issue").setValue("packaging arrived crushed");
    await $("#c-issue-send").click();
    await publishOk();
    await browser.waitUntil(async () =>
      (await $("#fb-issues").getText()).includes("packaging arrived crushed")
    );
  });
});

describe("my feedback (the local journal)", () => {
  it("lists every publish, newest first, all live", async () => {
    await openJournal();
    await expect($$("#journal-list li")).toBeElementsArrayOfSize(5);
    // Newest first: issue, thumb, tag, comment, stars.
    const kinds = await $$("#journal-list li").map((r) => r.getAttribute("data-kind"));
    expect(kinds).toEqual(["issue", "thumb", "tag", "comment", "stars"]);
    const statuses = await $$("#journal-list li .journal-status").map((s) => s.getText());
    expect(statuses).toEqual(["live", "live", "live", "live", "live"]);
  });

  it("updates an entry by supersession (same key + target, newest wins)", async () => {
    await openJournal();
    const starsRow = await $('#journal-list li[data-kind="stars"]');
    await starsRow.$(".journal-update").click();
    const editor = starsRow.$(".journal-editor");
    await editor.waitForDisplayed();
    await editor.$("select").selectByAttribute("value", "5");
    await editor.$(".journal-save").click();

    // The journal re-renders: a new live stars row + the superseded original.
    await browser.waitUntil(
      async () => (await $$("#journal-list li").getElements()).length === 6
    );
    const statuses = await $$('#journal-list li[data-kind="stars"] .journal-status').map(
      (s) => s.getText()
    );
    expect(statuses.sort()).toEqual(["live", "superseded"]);
  });

  it("erases an entry: two-step confirm, server forgets it (ADR 0021)", async () => {
    await openJournal();
    const commentRow = await $('#journal-list li[data-kind="comment"]');
    const del = commentRow.$(".journal-delete");
    await del.click();
    await expect(del).toHaveText("Really delete?");
    await del.click();

    await browser.waitUntil(async () => {
      const row = await $('#journal-list li[data-kind="comment"]');
      return (await row.$(".journal-status").getText()) === "deleted";
    });

    // The Feedback screen no longer shows the erased comment.
    await lookUp("3017620422003");
    await waitView("feedback");
    await browser.waitUntil(async () =>
      (await $("#fb-comments").getText()).includes("No comments yet.")
    );
  });
});

describe("my key", () => {
  before(async () => {
    await goHome();
    await $("#nav-key").click();
    await waitView("key");
  });

  it("refuses a garbage PEM with a typed error (key preserved)", async () => {
    await $("#key-import").setValue("not a key at all");
    const btn = await $("#key-import-btn");
    await btn.click(); // arm
    await btn.click(); // confirm
    const error = await $("#key-error");
    await error.waitForDisplayed();
    await expect(error).toHaveText(expect.stringContaining("invalid key PEM"));
  });

  it("re-imports the exported PEM (portable account)", async () => {
    expect(exportedPem).toContain("BEGIN PRIVATE KEY");
    // Typing a multi-line PEM through synthetic keys is unreliable (newlines
    // act as form submits in some drivers); set the textarea value directly
    // and fire the input event, as a paste would.
    await browser.execute((pem) => {
      const el = document.getElementById("key-import");
      el.value = pem;
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }, exportedPem);
    const btn = await $("#key-import-btn");
    await btn.click(); // arm
    await btn.click(); // confirm
    await browser.waitUntil(
      async () =>
        (await $("#key-ok").isDisplayed()) || (await $("#key-error").isDisplayed()),
      { timeoutMsg: "neither key-ok nor key-error appeared" }
    );
    if (await $("#key-error").isDisplayed()) {
      throw new Error(`import failed: ${await $("#key-error").getText()}`);
    }
    await expect($("#key-ok")).toHaveText(expect.stringContaining("Imported"));
  });
});

describe("key backup nudge", () => {
  it("arms once the key needs backing up again (already 3+ posts on the journal)", async () => {
    // "my key" just re-imported the exported PEM, which re-arms the nudge —
    // the journal already has several posts from earlier blocks.
    await goHome();
    await $("#backup-nudge").waitForDisplayed();
  });

  it("clears once the key is exported again", async () => {
    await $("#nav-key").click();
    await waitView("key");
    await $("#key-export-btn").click();
    await browser.waitUntil(async () =>
      (await $("#key-export").getValue()).includes("BEGIN PRIVATE KEY")
    );
    await goHome();
    await expect($("#backup-nudge")).not.toBeDisplayed();
  });
});

describe("settings", () => {
  it("shows the configured server and saves changes", async () => {
    await goHome();
    await $("#nav-settings").click();
    await waitView("settings");
    await expect($("#settings-server")).toHaveValue(SERVER_URL);

    await $("#settings-server").setValue(SERVER_URL);
    await $("#settings-save").click();
    await $("#settings-ok").waitForDisplayed();
  });
});

describe("author view (fingerprint badge on a comment)", () => {
  before(async () => {
    // Self-contained: publish a fresh comment rather than relying on state
    // from earlier blocks, some of which erase their own comments.
    await lookUp("3017620422003");
    await waitView("feedback");
    await $("#c-comment").setValue("solid choice");
    await $("#c-comment-send").click();
    await publishOk();
    await browser.waitUntil(async () =>
      (await $("#fb-comments").getText()).includes("solid choice")
    );
  });

  it("shows a tappable fingerprint badge next to the comment", async () => {
    const badge = await $("#fb-comments .fb-fp");
    await expect(badge).toBeDisplayed();
    await expect(badge).toHaveText(expect.stringMatching(/^#[0-9a-f]{8}$/));
  });

  it("opens the author's own feedback screen (their identity IRI as target)", async () => {
    const badge = await $("#fb-comments .fb-fp");
    const issuerId = await badge.getAttribute("title");
    await badge.click();
    await waitView("author");
    await expect($("#author-id")).toHaveText(issuerId);
  });

  it("lets you leave a note on the author, then Back returns to the product", async () => {
    await $("#a-comment").setValue("thanks for the tip!");
    await $("#a-comment-send").click();
    await $("#author-ok").waitForDisplayed();
    await browser.waitUntil(async () =>
      (await $("#author-comments").getText()).includes("thanks for the tip!")
    );

    await $("#nav-back").click();
    await waitView("feedback");
    await expect($("#fb-target")).toHaveText(GS1_NUTELLA);
  });
});
