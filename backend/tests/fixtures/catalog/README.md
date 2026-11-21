# Synthetic GameBanana catalog fixtures

These fixtures are authored for Modlock. The submission IDs, image paths,
titles and descriptions are invented. The provider hosts, API field names and
Deadlock game ID 20948 are protocol identifiers. The tests run offline.

- `large-gallery.json` exercises a 39-image provider response. The normalizer
  keeps the first 32 in order, retains the first entry's thumbnail and
  validates the entire input, including the tail.
- `empty-collection.json` exercises an empty collection with a numeric item
  count of `-1`. The normalizer accepts that value for collection counts.
- `styled-list.html` and `inline-media.html` cover provider color classes, text
  styles, lists, relative links, dividers and allowed image URLs. An iframe
  checks that embedded players are discarded.
