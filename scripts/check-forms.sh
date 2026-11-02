#!/usr/bin/env bash
# Hands the forms that the engine filled in and saved to tools that are not PDFium, to see what
# other readers see:
#
#   - qpdf checks the structure of the saved file (nothing it complains about, not even a warning)
#     and unpacks it, so that the appearance streams can be read
#   - pdftk reads the field values the way a form tool does
#   - poppler draws the page, and the drawing of each field that was filled in must differ from the
#     drawing of the empty form: if the appearance streams were stale, the page would look empty
#
# usage: scripts/check-forms.sh <folder>
#
# The folder is the one the test `the_files_for_other_tools_are_written` of the engine fills when
# PARTRIDGE_FORMS_OUT names it:
#
#   PARTRIDGE_FORMS_OUT=/tmp/forms cargo test -p partridge-engine --test forms
#   scripts/check-forms.sh /tmp/forms
#
# Needs qpdf, pdftk (pdftk-java) and poppler-utils (pdftoppm). On Debian and Ubuntu:
#   apt-get install qpdf pdftk-java poppler-utils
set -euo pipefail

dir=${1:?usage: check-forms.sh <folder>}
cd "$dir"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

for tool in qpdf pdftk pdftoppm; do
  command -v "$tool" >/dev/null || fail "$tool is not installed"
done

# ---- structure --------------------------------------------------------------------------------
for file in filled nested-filled; do
  if ! qpdf --check "$file.pdf" >"$file.check" 2>&1; then
    cat "$file.check" >&2
    fail "qpdf --check $file.pdf"
  fi
  grep -q "No syntax or stream encoding errors found" "$file.check" || fail "qpdf did not say $file.pdf is clean"
  if grep -qi "warning" "$file.check"; then
    cat "$file.check" >&2
    fail "qpdf warns about $file.pdf"
  fi
  # The same file with the streams unpacked and the objects in the open, to read the appearances.
  qpdf --qdf --object-streams=disable "$file.pdf" "$file.qdf"
done
echo "ok: qpdf finds no problem in the saved files"

# ---- values, as a form tool reads them --------------------------------------------------------
# pdftk prints a block for each field; the output is turned into `name<TAB>value` lines, one for
# each value (a list box with two choices has two). A value of several lines gives its first line.
values() {
  pdftk "$1" dump_data_fields_utf8 | awk '
    /^FieldName: /  { name = substr($0, 12) }
    /^FieldValue: / { value = substr($0, 13); gsub(/\r/, "", value); print name "\t" value }'
}

check_values() {
  local pdf=$1 expected=$2
  values "$pdf" >"$pdf.values"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    if ! grep -Fxq -- "$line" "$pdf.values"; then
      echo "--- what pdftk read from $pdf:" >&2
      cat "$pdf.values" >&2
      fail "pdftk did not read the value '$line' from $pdf"
    fi
  done <"$expected"
}

check_values filled.pdf expected.txt
check_values nested-filled.pdf nested-expected.txt
# The same form as the engine in its sandbox saved it (the test writes it next to the others).
if [ -f sandbox-first.pdf ]; then
  qpdf --check sandbox-first.pdf >sandbox-first.check 2>&1 || { cat sandbox-first.check >&2; fail "qpdf --check sandbox-first.pdf"; }
  check_values sandbox-first.pdf expected.txt
else
  fail "the test did not write sandbox-first.pdf"
fi
# A value of more than one line: pdftk prints the second line on a line of its own.
pdftk filled.pdf dump_data_fields_utf8 | grep -q "^Second line" || fail "pdftk did not read the second line of the notes"
echo "ok: pdftk reads every value that was set"

# ---- appearance streams, read from the unpacked file ------------------------------------------
# What another reader draws is the appearance stream of the widget. The text of a field must be in
# it, whatever the value in the dictionary says.
for text in "Ada Lovelace" "First line" "Second line" "Sweden" "Troms"; do
  grep -aFq -- "($text" filled.qdf || fail "no appearance stream shows '$text'"
done
echo "ok: the appearance streams carry the text"

# ---- drawing ----------------------------------------------------------------------------------
# 72 dots per inch, so a point is a pixel. A box is `x y width height` from the top left of the
# page (the places of the fields in the test form, see crates/testkit/src/forms.rs). The empty form
# and the filled one are drawn, and each box must differ between them. The empty form drawn twice
# gives the same box, so a difference is the filling and not the drawing.
box_of() { # file name x y w h -> the file that holds the box
  local out="box-${1%.pdf}-$2"
  pdftoppm -r 72 -f 1 -l 1 -x "$3" -y "$4" -W "$5" -H "$6" -singlefile "$1" "$out"
  [ -s "$out.ppm" ] || fail "poppler drew nothing for the box $2 of $1"
  echo "$out.ppm"
}
# fullname: 150..400 across, 68..92 down. notes: 150..450 across, 108..192 down. agree: 150..166,
# 248..264 down.
for box in "fullname 150 68 250 24" "notes 150 108 300 84" "agree 150 248 16 16"; do
  # shellcheck disable=SC2086
  set -- $box
  empty=$(box_of empty.pdf "$@")
  again=$(box_of empty.pdf "$@")
  filled=$(box_of filled.pdf "$@")
  cmp -s "$empty" "$again" || fail "poppler draws the empty box $1 differently each time"
  if cmp -s "$empty" "$filled"; then
    fail "poppler draws the box of $1 the same as in the empty form"
  fi
done
echo "ok: poppler draws what was filled in"
