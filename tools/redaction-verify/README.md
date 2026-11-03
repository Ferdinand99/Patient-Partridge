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
   stack of `q` and `Q`, the matrix, the clip, the colours, the dash pattern, the miter limit, the text
   state with spacing, scale, leading, rise and font, the text and line matrices) and follows forms
   (with `/Matrix` and `/BBox`), soft mask groups, every state of every annotation appearance, the
   icons of widgets and the glyph procedures of Type 3 fonts. Every painting event of a page inside
   the strict profile (below) is placed on the page: a glyph of any font, the fill or stroke of a
   path, a clip path, a shading, and what a Type 3 glyph paints. One that meets the region is a leak
   whatever its colour, clip, transparency, blend mode, render mode or optional content.

   **The strict profile.** The geometry check gives exit status 0 only when every page, with the
   forms, appearances, patterns, soft mask groups and Type 3 procedures that it draws, lies inside
   a profile that the tool understands fully. What is outside it is "could not check", never a
   pass: the tool does not try to model what other readers do with a feature, it leaves the feature
   out. The profile is one module, `src/profile.rs`, and this section:

   - **Operators.** Every operator is one of the specification (Table A.1) and has exactly the
     number and the types of operands that the specification gives: finite numbers, whole numbers
     where one is wanted (`j`, `J`, `Tr`), a `TJ` that shows only strings and numbers. An operator
     with more operands is read by Poppler from its last ones, and by others from its first, so it
     is refused; so is an unknown operator, unless it is between `BX` and `EX`. The text layers read
     the operands the same way. A number that a strict reader does not accept (`1+19000`, `110.-0`,
     `1e5`), a name with a bad escape and a string that does not end are refused too.
   - **Matrices and boxes.** A `/Matrix`, `/BBox`, `/Rect`, `/MediaBox` and `/FontMatrix` is exactly
     six (a matrix) or four (a box) finite numbers. A form has a `/BBox`.
   - **Fonts.** Text is shown only with
     - one of the standard fonts (Helvetica and Arial, Times, Courier) that the file does not
       embed, whose `/Encoding` is absent, `WinAnsiEncoding` or `MacRomanEncoding` (a name; no
       `/Differences`, no base encoding), and whose `/Widths`, if it has any, are within 1 % of the
       standard widths for every code from 32 to 126 and fit `/FirstChar`..`/LastChar`. A code
       outside 32 to 126 of such a font with `/Widths` is refused; with none, the tool places it
       by an interval (the widths of the 14 fonts are what Poppler uses);
     - a simple font (Type 1, MMType1, TrueType) whose program is in the file (TrueType, OpenType,
       CFF or Type 1) and whose bounding box the tool reads, with `/Widths` of exactly
       `/LastChar - /FirstChar + 1` numbers; a shown code outside that range, and a width outside
       0.01 to 3 em (the space may be 0), are refused. For a TrueType program every width must be
       the advance of some glyph of the program (2 %);
     - a composite font (Type 0) with the encoding `Identity-H` as a name, one descendant whose
       program is in the file, and, for a TrueType program, a width (`/W`, `/DW`) that is the
       advance of the glyph of the code (through `/CIDToGIDMap`);
     - a Type 3 font whose `/FontMatrix` has no rotation or skew, whose `/Widths` fit
       `/FirstChar`..`/LastChar`, and in which every shown code is named by `/Differences` and has
       a procedure that is walked.

     A font that is not embedded and not a standard font (also Symbol and ZapfDingbats), a program
     that the tool cannot read, an `/Encoding` that is a stream or a name other than `Identity-H`
     (every vertical map, a `/WMode` of 1 in the font or in a character map), and a width that
     disagrees with the program are refused. The box of the program (the `head` table, the
     `FontBBox` of a CFF or Type 1 program) must lie within 0.5 em above and below the cell of a
     glyph (the ascent and descent, see below) and within 1.2 em left of the origin and right of
     the widest advance: ink that is farther from its cell than that is refused.
   - **Annotations.** An annotation with the NoRotate flag on a page that is turned is refused when
     its rectangle, widened on every side by its longer side, meets a region (a viewer turns it back
     about a corner of its rectangle).
   - **Strokes.** The miter limit is the real one. A miter limit above 10, and a dash pattern on a
     path with joins (a rectangle, a polyline, a curve), are refused when the stroke is near a
     region.
   - **Budget.** A walk that needs more steps (`--walk-steps`, default 100 000 000) or time
     (`--walk-budget`, default 120 s) than its budget is refused, and so is the work that fonts,
     patterns, font programs and glyph procedures need when they are read.

   **The glyph rule** is the contract between this check and the redaction engine, and it is the
   same in every spec. The cell of a glyph is

   - across: from the glyph origin along the baseline by the width of the glyph times the font size
     times the horizontal scale (`Tz`). Character and word spacing (`Tc`, `Tw`) and kerning move the
     next glyph, not the cell;
   - up and down: from the descent to the ascent of the font, in em times the font size, with the
     rise (`Ts`). For a standard font these are -0.25 and 0.97 em. For an embedded font they are
     the `/Descent` and `/Ascent` of its descriptor, held within the box of the font program (the
     box itself where the descriptor has none). For a Type 3 font they are the vertical extent of
     its `/FontBBox`, or -0.2 to 1.0 em without one;
   - moved by the text matrix and the matrix of the page; the bounds of the cell on the page are the
     bounds of its four corners.

   **A glyph is under a region when its cell overlaps the region with positive area**: it reaches
   more than 0.01 point into the region both across and up and down. A glyph whose cell only
   touches the region, or ends where the region starts to within that, is not under it. A space is
   a glyph like any other. A glyph of a known place that is under a region is a leak, whatever the
   spec looks for. Where the width of a glyph is not known (a code of a standard font that the
   tables do not have), the place is an interval and the widest cell it can have counts.

   **The engine must remove every glyph whose cell overlaps the region by this rule**, and no more
   is needed: the glyph before a word that ends where the region starts stays (a word box that is
   the region exactly), and the space before a word stays only when the region does not reach into
   it (with a region 1 point larger than the word, the spaces next to the word and the end of a
   narrow letter overlap it, and go with the word). The rule is the same for the glyphs of a
   neighbouring line whose cell reaches the region. The ink of a glyph that is outside its cell is
   not counted, within the limits of the profile above.

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
   - Nothing is dropped. Every event is tested against the regions as it happens.

Name the fill colour: without it a region over one image sample, or a box of any colour, cannot be
told from a region that was not redacted.

## Known limits

- A composite font without `ToUnicode`, an embedded symbolic font without an encoding, and a Type 3
  font whose glyph names are not glyph names make the text shown with them "could not check" in the
  text layers: the font program is not parsed for characters. A real file with such a font cannot be
  the input of a redaction test.
- A `ToUnicode` map into a private use area (U+E000 to U+F8FF, planes 15 and 16) says nothing about
  the glyph and is read as unmapped.
- A phrase over the text of a page and the text of a form it draws, or over two annotation
  appearances, is not joined. A word that one glyph of a Type 3 font draws is a gap of at most three
  characters.
- The cell of a glyph is its advance box and the ascent and descent of its font. The ink of a glyph
  that lies outside its cell is not counted, but the profile bounds how far it can be (the box of the
  font program, 0.5 em above and below the cell, 1.2 em across). A text on the line next to a word
  can meet the region when its cell does.
- The widths of a simple font are checked against the program only as a set, and only for a
  TrueType program (every width is the advance of some glyph): the code is not followed to its glyph.
  The widths of a CFF or Type 1 program are not read, and are the word of the file within 0.01 to 3
  em. A composite font with a CFF program is placed by its `/W` as the file gives it.
- Geometry is placed by its bounds, and bounds err on the side of a leak: a curve is bounded by its
  control points, a stroke of straight segments by the segments and the widest line a viewer draws.
  A line a few points from a region can be a leak.
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
