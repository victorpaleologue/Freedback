// Barcode demo — the original 2014 white-book use case, live (classic script,
// NOT a module).
//
// A product barcode (EAN-13, UPC-A/GTIN-12, EAN-8, GTIN-14) is validated with
// the standard GS1 mod-10 check digit, normalized to GTIN-14, and turned into
// its GS1 Digital Link URI `https://id.gs1.org/01/<gtin14>` — the product's
// standard identity on the web. The shipped widgets
// (/widgets/freedback-widgets.js, loaded BEFORE this script) are then bound to
// that URI against the LIVE demo server. No mock backend on this page.
//
// The widget elements are created DETACHED and appended only after every
// data-* attribute is set: connectedCallback runs render()+refresh() the
// moment the element is connected, so attributes set afterwards would be
// ignored (same rule demo-react/src/FreedbackWidget.jsx documents for React).
//
// Pure GTIN helpers live in `FreedbackGtin` and are unit-testable under Node:
//   node --check site/barcode/barcode.js      # syntax
//   node site/barcode/barcode.js --test       # self-test (no DOM required)
(function (global) {
  "use strict";

  // --- Pure GTIN logic (no DOM) --------------------------------------------

  var GTIN_LENGTHS = { 8: "EAN-8", 12: "UPC-A / GTIN-12", 13: "EAN-13", 14: "GTIN-14" };

  /** GS1 mod-10 check digit for a string of digits WITHOUT its check digit.
   *  Weights are anchored at the right: the digit immediately left of the
   *  check digit weighs 3, then 1, 3, 1, ... (leading zeros weigh nothing,
   *  which is why zero-padding to GTIN-14 preserves validity). */
  function gs1CheckDigit(digitsWithoutCheck) {
    var sum = 0;
    for (var i = 0; i < digitsWithoutCheck.length; i++) {
      var digit = digitsWithoutCheck.charCodeAt(digitsWithoutCheck.length - 1 - i) - 48;
      sum += digit * (i % 2 === 0 ? 3 : 1);
    }
    return (10 - (sum % 10)) % 10;
  }

  /** Validate user input as a GTIN. Returns
   *  `{ ok: true, gtin14, kind }` or `{ ok: false, error }`. */
  function validateGtin(input) {
    var raw = String(input == null ? "" : input).trim();
    if (raw === "") return { ok: false, error: "Enter the digits printed under the barcode." };
    if (!/^[0-9]+$/.test(raw)) {
      return { ok: false, error: "Digits only, please — no letters, spaces or dashes." };
    }
    var kind = GTIN_LENGTHS[raw.length];
    if (!kind) {
      return {
        ok: false,
        error: "A barcode has 8, 12, 13 or 14 digits (got " + raw.length + ").",
      };
    }
    var expected = gs1CheckDigit(raw.slice(0, -1));
    var actual = raw.charCodeAt(raw.length - 1) - 48;
    if (expected !== actual) {
      return {
        ok: false,
        error:
          "Check digit mismatch: a " + kind + " ending in " + actual +
          " should end in " + expected + ". Please re-check the digits.",
      };
    }
    var gtin14 = raw;
    while (gtin14.length < 14) gtin14 = "0" + gtin14;
    return { ok: true, gtin14: gtin14, kind: kind };
  }

  /** GS1 Digital Link URI — the product's standard identity on the web. */
  function gs1DigitalLink(gtin14) {
    return "https://id.gs1.org/01/" + gtin14;
  }

  var FreedbackGtin = {
    gs1CheckDigit: gs1CheckDigit,
    validateGtin: validateGtin,
    gs1DigitalLink: gs1DigitalLink,
  };
  global.FreedbackGtin = FreedbackGtin;

  // --- Self-test (Node: `node site/barcode/barcode.js --test`) -------------

  function selfTest() {
    function assert(cond, label) {
      if (!cond) throw new Error("FreedbackGtin self-test FAILED: " + label);
    }
    // Valid barcodes of every accepted length.
    var valid = [
      ["3017620422003", "03017620422003", "EAN-13"], // hazelnut spread
      ["5449000000996", "05449000000996", "EAN-13"], // cola
      ["4006381333931", "04006381333931", "EAN-13"], // highlighter
      ["96385074", "00000096385074", "EAN-8"],
      ["036000291452", "00036000291452", "UPC-A / GTIN-12"],
      ["10614141543219", "10614141543219", "GTIN-14"],
    ];
    for (var i = 0; i < valid.length; i++) {
      var v = validateGtin(valid[i][0]);
      assert(v.ok, valid[i][0] + " should be valid: " + (v.error || ""));
      assert(v.gtin14 === valid[i][1], valid[i][0] + " → " + v.gtin14 + " ≠ " + valid[i][1]);
      assert(v.kind === valid[i][2], valid[i][0] + " kind " + v.kind + " ≠ " + valid[i][2]);
    }
    // Zero-padding preserves the check digit: GTIN-14 form of an EAN-13 is valid too.
    assert(validateGtin("03017620422003").ok, "zero-padded EAN-13 stays valid");
    // Invalid: bad check digit, bad length, non-digits, empty.
    assert(!validateGtin("3017620422004").ok, "flipped check digit must fail");
    assert(!validateGtin("12345").ok, "wrong length must fail");
    assert(!validateGtin("30176204ZZ003").ok, "letters must fail");
    assert(!validateGtin("  ").ok, "blank must fail");
    // Digital Link.
    assert(
      gs1DigitalLink("03017620422003") === "https://id.gs1.org/01/03017620422003",
      "digital link URI"
    );
    return valid.length + 6;
  }

  var isNode =
    typeof process !== "undefined" && process.versions && process.versions.node &&
    typeof document === "undefined";
  if (isNode && process.argv.indexOf("--test") !== -1) {
    var n = selfTest();
    console.log("FreedbackGtin self-test OK (" + n + " checks)");
  }
  FreedbackGtin.selfTest = selfTest;

  // --- Page wiring (browser only) -------------------------------------------

  if (typeof document === "undefined") return;

  var LIVE_ANNOTATIONS = "https://freedback-demo.fly.dev/annotations/";
  var LICENSE = "https://creativecommons.org/licenses/by/4.0/";

  /** Create one shipped widget with ALL attributes set while still detached,
   *  then hand it back for appending (connectedCallback timing rule). */
  function makeWidget(kind, target) {
    var el = document.createElement("freedback-" + kind);
    el.setAttribute("data-target", target);
    el.setAttribute("data-read", LIVE_ANNOTATIONS);
    el.setAttribute("data-publish", LIVE_ANNOTATIONS);
    el.setAttribute("data-sign", ""); // presence = self-signed P-256 identity
    el.setAttribute("data-license", LICENSE);
    el.setAttribute("data-author-href", "/author/");
    return el;
  }

  function showProduct(gtin14) {
    var uri = gs1DigitalLink(gtin14);
    var card = document.getElementById("product-card");
    var uriEl = document.getElementById("target-uri");
    var host = document.getElementById("product-widgets");
    uriEl.textContent = uri;

    // Rebuild the widgets for the new target (attributes before append).
    host.replaceChildren();
    var starsRow = document.createElement("div");
    var starsLabel = document.createElement("span");
    starsLabel.textContent = "Rate this product: ";
    starsRow.appendChild(starsLabel);
    starsRow.appendChild(makeWidget("stars", uri));
    host.appendChild(starsRow);
    host.appendChild(makeWidget("comment", uri + "#comments"));
    host.appendChild(makeWidget("tag", uri + "#tags"));

    card.hidden = false;
    card.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  function init() {
    var form = document.getElementById("gtin-form");
    var input = document.getElementById("gtin-input");
    var errorEl = document.getElementById("gtin-error");
    var scanBtn = document.getElementById("scan-btn");
    var scanNote = document.getElementById("scan-note");
    var scanner = document.getElementById("scanner");

    function setError(msg) {
      errorEl.textContent = msg || "";
      errorEl.hidden = !msg;
    }

    function submitGtin(raw) {
      var v = validateGtin(raw);
      if (!v.ok) {
        setError(v.error);
        document.getElementById("product-card").hidden = true;
        return;
      }
      setError("");
      showProduct(v.gtin14);
    }

    form.addEventListener("submit", function (ev) {
      ev.preventDefault();
      submitGtin(input.value);
    });

    // Example chips: fill the field and go.
    var chips = document.querySelectorAll("#examples [data-gtin]");
    for (var i = 0; i < chips.length; i++) {
      chips[i].addEventListener("click", function (ev) {
        input.value = ev.currentTarget.getAttribute("data-gtin");
        submitGtin(input.value);
      });
    }

    // --- Camera scanning (feature-detected; Chrome/Android) ----------------
    var supported =
      "BarcodeDetector" in window &&
      !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia);
    if (!supported) {
      scanBtn.hidden = true;
      scanNote.hidden = false;
      return;
    }
    scanBtn.hidden = false;

    var activeStream = null;
    var scanTimer = null;

    function stopScan() {
      if (scanTimer) { clearInterval(scanTimer); scanTimer = null; }
      if (activeStream) {
        activeStream.getTracks().forEach(function (t) { t.stop(); });
        activeStream = null;
      }
      scanner.replaceChildren();
      scanner.hidden = true;
      scanBtn.textContent = "Scan with camera";
    }

    function startScan() {
      var detector;
      try {
        detector = new window.BarcodeDetector({ formats: ["ean_13", "ean_8", "upc_a"] });
      } catch (e) {
        setError("This browser exposes BarcodeDetector but cannot construct it: " + e.message);
        return;
      }
      navigator.mediaDevices
        .getUserMedia({ video: { facingMode: "environment" }, audio: false })
        .then(function (stream) {
          activeStream = stream;
          var video = document.createElement("video");
          video.setAttribute("playsinline", ""); // iOS: no fullscreen takeover
          video.muted = true;
          video.srcObject = stream;
          video.style.maxWidth = "100%";
          scanner.replaceChildren(video);
          scanner.hidden = false;
          scanBtn.textContent = "Stop scanning";
          return video.play().then(function () {
            scanTimer = setInterval(function () {
              detector
                .detect(video)
                .then(function (codes) {
                  if (!codes || !codes.length) return;
                  var value = codes[0].rawValue;
                  input.value = value;
                  stopScan();
                  submitGtin(value);
                })
                .catch(function () { /* transient decode error: keep polling */ });
            }, 200);
          });
        })
        .catch(function (err) {
          stopScan();
          setError("Camera unavailable: " + (err && err.message ? err.message : err));
        });
    }

    scanBtn.addEventListener("click", function () {
      if (activeStream) stopScan();
      else { setError(""); startScan(); }
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})(typeof window !== "undefined" ? window : globalThis);
