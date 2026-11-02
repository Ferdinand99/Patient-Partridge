#!/usr/bin/env bash
# Hands the documents that the page tools changed and saved to tools that are not PDFium, to see
# what other readers see:
#
#   - qpdf checks the structure of each saved file (nothing it complains about, not even a warning)
#     and unpacks it, so that the bytes of a deleted page can be searched for
#   - poppler (pdfinfo, pdftotext, pdftoppm) reads the page count, the order of the pages, the
#     turn, the crop box and the size of the pages, and draws every page
#
# usage: scripts/check-pages.sh <folder>
#
# The folder is the one the test `the_files_for_other_tools_are_written` of the engine fills when
# PARTRIDGE_PAGES_OUT names it, with `expected.txt` (one line per file: name, number of pages and
# what to find):
#
#   PARTRIDGE_PAGES_OUT=/tmp/pages cargo test -p partridge-engine --test pages_process
#   scripts/check-pages.sh /tmp/pages
#
# Needs qpdf and poppler-utils. On Debian and Ubuntu: apt-get install qpdf poppler-utils
set -euo pipefail

dir=${1:?usage: check-pages.sh <folder>}
cd "$dir"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

for tool in qpdf pdfinfo pdftotext pdftoppm od; do
  command -v "$tool" >/dev/null || fail "$tool is not installed"
done
[ -s expected.txt ] || fail "no expected.txt: the test did not write the files"

# The text of a page as the ASCIIHex filter writes it, in capitals.
hex_of() { printf '%s' "$1" | od -An -tx1 -v | tr -d ' \n' | tr 'a-f' 'A-F'; }

# The marker of page $1 of the made-up document, in both forms.
marker() { printf 'PAGE-TEXT-%s-SECRET' "$1"; }

page_info() { # file page -> the line pdfinfo prints for that page and key
  pdfinfo -f "$2" -l "$2" "$1"
}

count=0
while IFS=$'\t' read -r name pages note; do
  [ -n "$name" ] || continue
  count=$((count + 1))

  # ---- structure -------------------------------------------------------------------------------
  if ! qpdf --check "$name" >"$name.check" 2>&1; then
    cat "$name.check" >&2
    fail "qpdf --check $name"
  fi
  grep -q "No syntax or stream encoding errors found" "$name.check" || fail "qpdf did not say $name is clean"
  if grep -qi "warning" "$name.check"; then
    cat "$name.check" >&2
    fail "qpdf warns about $name"
  fi
  qpdf --qdf --object-streams=disable "$name" "$name.qdf"

  # ---- pages -----------------------------------------------------------------------------------
  got=$(pdfinfo "$name" | awk '/^Pages:/ { print $2 }')
  [ "$got" = "$pages" ] || fail "$name: poppler counts $got pages, expected $pages"
  # Every page can be drawn.
  rm -f "draw-$name"-*.png
  pdftoppm -r 20 -png "$name" "draw-$name"
  drawn=$(ls "draw-$name"-*.png | wc -l)
  [ "$drawn" = "$pages" ] || fail "$name: poppler drew $drawn of $pages pages"

  # ---- what to find ----------------------------------------------------------------------------
  for item in $note; do
    key=${item%%=*}
    value=${item#*=}
    case "$key" in
      text)
        # The marker of the page that stands at each place, in order; x is a place with no marker.
        i=1
        for p in ${value//,/ }; do
          shown=$(pdftotext -f "$i" -l "$i" "$name" -)
          if [ "$p" = x ]; then
            if grep -q 'PAGE-TEXT-' <<<"$shown"; then fail "$name: place $i should have no marker"; fi
          else
            grep -q "$(marker "$p")" <<<"$shown" || fail "$name: place $i does not show $(marker "$p")"
            # qpdf unpacks the ASCIIHex streams of the made-up pages, so the text is there as it is
            # (and, were it not unpacked, in its hexadecimal form).
            grep -aq "$(marker "$p")" "$name.qdf" || grep -aq "$(hex_of "$(marker "$p")")" "$name.qdf" \n              || fail "$name: the file does not hold the text of page $p"
          fi
          i=$((i + 1))
        done
        ;;
      gone)
        # Not in the text and not in the bytes of the file, in either form.
        for p in ${value//,/ }; do
          if grep -aq "$(marker "$p")" "$name.qdf"; then fail "$name: the file still has the text of deleted page $p"; fi
          if grep -aq "$(hex_of "$(marker "$p")")" "$name.qdf"; then fail "$name: the file still has the text of deleted page $p (hex)"; fi
          if grep -aq "NOTE-$p-SECRET" "$name.qdf"; then fail "$name: the file still has the note of page $p"; fi
          if grep -aq "FontSecret$p" "$name.qdf"; then fail "$name: the file still has the font of page $p"; fi
          if grep -aq "$(hex_of "IMAGE-DATA-$p-SECRET")" "$name.qdf"; then fail "$name: the file still has the picture of page $p"; fi
        done
        ;;
      rotation)
        i=1
        for r in ${value//,/ }; do
          got=$(page_info "$name" "$i" | awk -v n="$i" '$1 == "Page" && $2 == n && $3 == "rot:" { print $4 }')
          [ "$got" = "$r" ] || fail "$name: page $i is turned $got, expected $r"
          i=$((i + 1))
        done
        ;;
      cropbox)
        want=${value//,/ }
        got=$(pdfinfo -f 1 -l 1 -box "$name" | awk '$1 == "Page" && $3 == "CropBox:" { print $4, $5, $6, $7 }')
        [ -n "$got" ] || fail "$name: poppler finds no crop box on page 1"
        # Compare as numbers: poppler prints two decimals.
        awk -v a="$got" -v b="$want" 'BEGIN { split(a, x, " "); split(b, y, " "); for (k = 1; k <= 4; k++) if (x[k] - y[k] > 0.01 || y[k] - x[k] > 0.01) exit 1 }' \
          || fail "$name: crop box is $got, expected $want"
        # And the media box did not change.
        media=$(pdfinfo -f 1 -l 1 -box "$name" | awk '$1 == "Page" && $3 == "MediaBox:" { print $4, $5, $6, $7 }')
        [ "$media" = "0.00 0.00 612.00 792.00" ] || fail "$name: the media box changed: $media"
        ;;
      size)
        got=$(page_info "$name" 1 | awk '$1 == "Page" && $3 == "size:" { print $4 "x" $6 }')
        [ "$got" = "$value" ] || fail "$name: page 1 is $got, expected $value"
        ;;
      blank)
        place=$((value + 1))
        shown=$(pdftotext -f "$place" -l "$place" "$name" -)
        if grep -q 'PAGE-TEXT-' <<<"$shown"; then fail "$name: place $place is not blank"; fi
        ;;
      size1)
        place=2
        got=$(page_info "$name" "$place" | awk -v n="$place" '$1 == "Page" && $2 == n && $3 == "size:" { print $4 "x" $6 }')
        [ "$got" = "$value" ] || fail "$name: place $place is $got, expected $value"
        ;;
      *) fail "$name: the test wrote a note this script does not know: $item" ;;
    esac
  done
  echo "ok: $name"
done <expected.txt

[ "$count" -ge 6 ] || fail "only $count files were checked"
echo "ok: qpdf and poppler read all $count files the page tools wrote"
