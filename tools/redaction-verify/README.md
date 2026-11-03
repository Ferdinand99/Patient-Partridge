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
used up, is killed together with everything it started, and the run ends with exit status 2 and a
message that names the tool and the setting. On Unix the tool runs in a process group of its own,
and the group is killed, also after a normal end. On Windows a job object holds the tool and all
its descendants, and ends them on a timeout and after a normal end. A process that starts a session
of its own on Unix (`setsid`) is out of reach. That holds even when another layer has found a leak:
the file was not fully checked, so no report is given. A value must be a number above 0 (a fraction
is allowed). In the library, `Options::tool_timeout` and `Options::file_budget` carry the same
settings.

## Regions

A region (`{"page": 1, "rect": [x0, y0, x1, y1], "fill": "#000000"}`) must pass three checks:

1. The page, rendered by Poppler at 144 dpi, is the fill colour inside the region (an edge of two
   device pixels is left out). With no fill named, it is one colour.
2. The images under the region, also turned ones, with their masks, are one value. A JPEG is
   compared only where whole blocks lie inside the region and the tolerance comes from the
   quantisation tables that its components use, capped at 12 levels (about libjpeg quality 15). A
   JPEG that cannot be judged (no whole block under the region, or tables too coarse) is "could not
   check".
3. The geometry. One interpreter walks the content of the page with a complete graphics state (the
   stack of `q` and `Q`, the matrix, the clip, the colours, the dash pattern, the text state with
   spacing, scale, leading, rise and font, the text and line matrices) and follows forms (with
   `/Matrix` and `/BBox`), soft mask groups, every state of every annotation appearance, the icons
   of widgets and the glyph procedures of Type 3 fonts. Every painting event is placed on the page:
   a glyph of any font, the fill or stroke of a path, a clip path, a shading, and what a Type 3
   glyph paints. One that meets the region is a leak whatever its colour, clip, transparency, blend
   mode, render mode or optional content.

   The rule above all: what the walk does not fully understand is "could not check", never a pass.
   It does not try to model every quirk of a reader; it refuses to judge. That covers a number that
   a strict reader does not accept (`1+19000`, `110.-0`, `1e5`), a name with a bad escape, a string
   that does not end, a name that cannot be found (a `Do`, `gs`, font, pattern or shading), an
   inline image whose end is in doubt, vertical writing, a Type 3 font that has no widths, no glyph
   names or a procedure that cannot be read, and a walk that needs more steps or time than its
   budget.

   - **Text.** A glyph is placed from the advance of each glyph (`/Widths`, or `/W` and `/DW`, or
     the standard font tables) and counts when **the centre of its cell is inside the region**, in
     every spec, whatever text the spec looks for. The cell is the larger of what the font says
     (the advance, the bounding box) and one em square from the glyph origin at the font size,
     scaled by the horizontal scale and the matrices; a font that the file describes can say
     anything about its metrics. The standard fonts that a file does not describe have no floor
     across, because their widths are the tables of the tool. Where the place of a glyph is not
     known (an interval of positions), the whole box counts.
     **The ink of a glyph that straddles the edge of the region, with less than half of its cell
     inside, is not counted.** This is the one place where the check is not strict: a page can hide
     the part of such a glyph that is outside the region, and the part inside is under the box. The
     engine must remove glyphs by the same rule (a glyph whose cell centre is in the region goes),
     so that the two agree.
   - An annotation that a viewer draws from its entries (`/Contents`, `/RC`, the value of a field)
     because it has no appearance stream is a leak when it has text and lies over the region, and
     "could not check" when it has none. A widget is drawn again from its value when the form says
     `/NeedAppearances true`, and when the widget has no appearance or an empty one: the same rule
     applies. An appearance with no `/Resources` uses those of the page, then the default
     resources of the form.
   - Geometry that carries nothing about what was under the region is allowed, when it is painted
     with no clip that cuts into the region and no soft mask: the box that redacts (one rectangle
     that is the region, in the colour that the region names); one rectangle in one device colour
     that holds the whole region (a page background, the shading of a cell); a horizontal or
     vertical line, or a rectangle, at most 2 points thick, that crosses the whole region from one
     side to the other (the rules of a table), when it is solid, has no constant alpha or blend
     mode, and is not clipped to the region on its own axis; a clip path that is one rectangle that
     holds the region. A clip that is anything else is geometry that meets the region. A subpath of
     one point is a zero-length segment: the dot of a round or square cap is geometry.
   - Nothing is dropped. Every event is tested against the regions as it happens. A page that needs
     more than `--walk-steps` steps (operators, glyphs, and what is read for a font, a pattern or a
     glyph procedure; default 100 000 000) or more than `--walk-budget` seconds (default 120) is
     "could not check".

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
- The ink of a glyph that straddles the edge of a region is not counted (see Regions, Text). Beyond
  that, bounds err on the side of a leak: a curve is bounded by its control points, a stroke of
  straight segments by the segments and the widest line a viewer draws, and a text on the line
  next to a word can meet the region when its cell does.
- A simple font with no `/Widths` is placed with the widths of the standard font that its name stands
  for (Helvetica, Arial, Times, Courier), within 1.5 %. A font that is not in the tables (Symbol,
  ZapfDingbats, anything else without widths) gives intervals and boxes that can be much wider than
  the text, and the whole box counts. The space of a font that is not in the file draws nothing and
  is no event.
- Vertical writing is "could not check", whenever such text is shown on a page with a region: the
  advance, the position vector (`/W2`, `/DW2`) and the spacing differ from horizontal text, and
  they are not placed.
- A rule that crosses a region still says where it is on one axis. The exception is limited to
  solid, opaque rules that no clip ends at the region on the rule's own axis.
- Geometry in objects that no page draws is not placed. The geometry that a tiling pattern paints
  is counted as the fill that uses it, not by the content of the pattern.
- The end of an inline image is taken from `/L` or `/Length`, from the size of the samples when the
  image has no filter, or else from the first `EI` that white space and content follow, as Poppler
  finds it. Where none of these fits, the image is "could not check". The regions are read from a
  copy of the file in which qpdf has not normalised the content, because qpdf reads the end of an
  inline image in its own way.
- Images drawn through a tiling pattern are checked by the render only, and the check says so. The
  images of a Type 3 glyph are placed with the glyph procedure.
- A region is compared at 144 dpi with an edge of two pixels, so a redaction whose edge is more than
  that off the region is a leak.

## Running the tests

`cargo test --locked` needs qpdf and Poppler (`pdftotext`, `pdftoppm`, `pdfimages`) on the path; the
test image in `Dockerfile` has the versions that CI uses. `scripts/run-in-docker.sh` builds and runs
the tests in a container, so that nothing heavy runs on the machine you work on.
