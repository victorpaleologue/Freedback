// Node unit tests for the pure widget helpers (no DOM needed).
// Run: node widgets/test.cjs
const assert = require("node:assert");
const { baseAnnotation, ratingValue, textBodies, readUrl } = require("./freedback-widgets.js");

// baseAnnotation emits the W3C wire shape.
const star = baseAnnotation("assessing", "https://ex/1", {
  type: ["freedback:StarRating", "schema:Rating"],
  "schema:ratingValue": 4,
  "schema:worstRating": 1,
  "schema:bestRating": 5,
});
assert.strictEqual(star.type, "Annotation");
assert.strictEqual(star.motivation, "assessing");
assert.strictEqual(star.conformsTo, "https://freedback.org/profile/1");
assert.ok(Array.isArray(star.body) && star.body.length === 1);
assert.ok(Array.isArray(star["@context"]));

// ratingValue pulls the numeric value out of a rating body.
assert.strictEqual(ratingValue(star), 4);
assert.strictEqual(
  ratingValue({ body: [{ type: "TextualBody", value: "hi", purpose: "commenting" }] }),
  null
);

// textBodies extracts comment/tag text by purpose.
const commented = {
  body: [{ type: "TextualBody", value: "nice", purpose: "commenting" }],
};
assert.deepStrictEqual(textBodies(commented, "commenting"), ["nice"]);
assert.deepStrictEqual(textBodies(commented, "tagging"), []);

// readUrl appends the encoded target.
assert.strictEqual(readUrl("http://h/index", "https://ex/1"), "http://h/index?target=https%3A%2F%2Fex%2F1");
assert.strictEqual(readUrl("http://h/index?x=1", "a"), "http://h/index?x=1&target=a");

console.log("widgets: all helper tests passed");
