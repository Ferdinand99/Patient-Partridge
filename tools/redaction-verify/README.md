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

## Time limits

Every call of qpdf or Poppler has a time limit, and all the calls for one file share a budget.

| setting | environment variable | flag | default |
|---|---|---|---|
| one call | `VERIFIER_TOOL_TIMEOUT` (seconds) | `--tool-timeout SECS` | 120 s |
| all calls for the file | `VERIFIER_FILE_BUDGET` (seconds) | `--file-budget SECS` | 900 s |

A flag wins over the variable. A call that runs over, or that is still running when the budget is
used up, is killed together with everything it started (on Unix it runs in a process group of its own, and the group is killed, also after a normal end; on Windows the process tree is killed with `taskkill /T`; a process that leaves the group on its own is out of reach), and
the run ends with exit status 2 and a message that names the tool and the setting. That holds even
when another layer has found a leak: the file was not fully checked, so no report is given. A value
must be a number above 0 (a fraction is allowed). In the library, `Options::tool_timeout` and
`Options::file_budget` carry the same settings.

## Regions

A region (`{"page": 1, "rect": [x0, y0, x1, y1], "fill": "#000000"}`) must pass three checks:

1. The page, rendered by Poppler at 144 dpi, is the fill colour inside the region (an edge of two
   device pixels is left out). With no fill named, it is one colour.
2. The images under the region, also turned ones, with their masks, are one value. A JPEG is
   compared only where whole blocks lie inside the region and the tolerance comes from the
   quantisation tables that its components use, capped at 12 levels (about libjpeg quality 15). A JPEG that cannot be judged
   (no whole block under the region, or tables too coarse) is "could not check".
3. The geometry. One interpreter walks the content of the page with a complete graphics state (the
   stack of `q` and `Q`, the matrix, the clip, the colours, the text state with spacing, scale,
   leading, rise and font, the text and line matrices) and follows forms (with `/Matrix` and
   `/BBox`), soft mask groups, every state of every annotation appearance and the icons of widgets.
   Every painting event is placed on the page: a glyph of any font, the fill or stroke of a path, a
   clip path, a shading. One that meets the region is a leak whatever its colour, clip,
   transparency, blend mode, render mode or optional content.

   - A glyph is placed from the advance of each glyph (`/Widths`, or `/W` and `/DW`) and its box
     from the bounding box of the font (the full em where the font has none that is plausible).
     Where a width is not known, the position of every later glyph of the string is an interval
     and the boxes cover it.
   - The glyphs of ordinary fonts are leaks when the spec has no text to look for. With a text in
     the spec, the text layers read them. The glyphs of a Type 3 font that paints are geometry
     either way.
   - An annotation that a viewer draws from its entries (`/Contents`, `/RC`, the value of a field)
     because it has no appearance stream is a leak when it has text and lies over the region, and
     "could not check" when it has none.
   - Geometry that carries nothing about what was under the region is allowed, when it is painted
     with no clip that cuts into the region and no soft mask: the box that redacts (one rectangle
     that is the region, in the colour that the region names); one rectangle in one device colour
     that holds the whole region (a page background, the shading of a cell); a horizontal or
     vertical line, or a rectangle, at most 2 points thick, that crosses the whole region from one
     side to the other (the rules of a table); a clip path that is one rectangle that holds the
     region. A clip that is anything else is geometry that meets the region.
   - Nothing is dropped. Every event is tested against the regions as it happens. A page that needs
     more than `--walk-steps` steps (operators and glyphs; default 100 000 000) or more than
     `--walk-budget` seconds (default 120) is "could not check".

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
  bounded by its control points, a stroke by the widest line a viewer draws, and a glyph by the
  box of its font, so text that ends at the edge of a region can meet it. A font with no widths
  (a standard font that the file does not describe) gives intervals, wider boxes and more false
  alarms.
- Images drawn through a tiling pattern or by Type 3 glyphs are checked by the render only, and the
  check says so.
- A region is compared at 144 dpi with an edge of two pixels, so a redaction whose edge is more than
  that off the region is a leak.

## Running the tests

`cargo test --locked` needs qpdf and Poppler (`pdftotext`, `pdftoppm`, `pdfimages`) on the path; the
test image in `Dockerfile` has the versions that CI uses. `scripts/run-in-docker.sh` builds and runs
the tests in a container, so that nothing heavy runs on the machine you work on.
