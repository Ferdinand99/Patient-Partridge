# redaction-verify

An independent check that redacted content is gone from a saved PDF. It is a test and CI tool: it
takes the saved file and a list of things that must not be in it, and looks for them everywhere a
PDF can keep a copy. It never calls PDFium and uses no PDF library, so a mistake in the code under
test cannot hide from it.

```
redaction-verify --spec spec.json file.pdf
redaction-verify --text "secret phrase" --region 1:72,400,122,450:0:000000 file.pdf
```

Exit status: 0 clean, 1 a leak was found, 2 the check could not be done. A tool that is missing or
fails, a file that qpdf cannot read, a stream or font that cannot be read, content whose bounds are
not known: each is a problem, and a report with a problem is never clean. Needle values are never
printed (a hit names the kind, the position and the start of a digest) unless `--show-values` is given.

## What it looks at

| layer       | what is searched                                                                  |
|-------------|-----------------------------------------------------------------------------------|
| `raw`       | the bytes of the file as written                                                  |
| `qdf`       | a copy that qpdf has normalised: streams decompressed, object streams opened      |
| `carve`     | every stream found by scanning the bytes (old revisions, orphans, object streams) |
| `decode`    | the text that every content stream shows, read through the fonts of the file      |
| `text`      | what Poppler `pdftotext` reads from the pages                                     |
| `structure` | the decoded strings, names and keys of the document, XMP, and forbidden keys      |
| `images`    | hashes of every image, and the redacted regions (render, images, geometry)        |
| `fonts`     | traces of removed text that stay in a subset font (only with `--fonts`)           |

`decode` reads each content stream in the order of the stream and in the order of the page (lines
from the top, then from the left), so words that were drawn in another order than they are read are
found. A word hyphenated at the end of a line is found too.

## Regions

A region (`{"page": 1, "rect": [x0, y0, x1, y1], "fill": "#000000"}`) must pass three checks:

1. The page, rendered by Poppler at 144 dpi, is the fill colour inside the region (an edge of two
   device pixels is left out). With no fill named, it is one colour.
2. The images under the region, also turned ones, with their masks, are one value. A JPEG is
   compared only where whole blocks lie inside the region and the tolerance comes from the
   quantisation tables that its components use, capped at 16 levels. A JPEG that cannot be judged
   (no whole block under the region, or tables too coarse) is "could not check".
3. No path, shading or Type 3 glyph is painted under the region, in the page, in forms, in every
   state of every annotation appearance, or in glyph procedures, whatever its colour, clip,
   transparency, blend mode or optional content. The only geometry allowed is the box that
   redacts: the fill of one rectangle that is the region, in the fill colour that the region names.

Name the fill colour: without it a region over one image sample, or a box of any colour, cannot be
told from a region that was not redacted.

## Known limits

- A composite font without `ToUnicode`, an embedded symbolic font without an encoding, and a Type 3
  font whose glyph names are not glyph names make the text shown with them "could not check": the
  font program is not parsed. A real file with such a font cannot be the input of a redaction test.
- A `ToUnicode` map into a private use area (U+E000 to U+F8FF, planes 15 and 16) says nothing about
  the glyph and is read as unmapped.
- A phrase over the text of a page and the text of a form it draws, or over two annotation
  appearances, is not joined. A word that one glyph of a Type 3 font draws is a gap of at most three
  characters.
- Geometry in objects that no page draws is not placed. Bounds err on the side of a leak: a curve is
  bounded by its control points, a stroke by the widest line a viewer draws, and a page background
  painted as a rectangle under a region is a leak unless it is the box.
- Images drawn through a tiling pattern or by Type 3 glyphs are checked by the render only, and the
  check says so.
- A region is compared at 144 dpi with an edge of two pixels, so a redaction whose edge is more than
  that off the region is a leak.

## Running the tests

`cargo test --locked` needs qpdf and Poppler (`pdftotext`, `pdftoppm`, `pdfimages`) on the path; the
test image in `Dockerfile` has the versions that CI uses. `scripts/run-in-docker.sh` builds and runs
the tests in a container, so that nothing heavy runs on the machine you work on.
