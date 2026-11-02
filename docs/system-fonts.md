# System fonts

A PDF can name a font without carrying it. Almost all PDFs for print or the web carry their fonts, and
for Latin text the reader has its own (Helvetica, Times, Courier and what is named like them). For
Chinese, Japanese, Korean, Arabic, Hebrew and Thai the reader needs a font of the computer, and this
page says which ones it looks for, and what that means for you.

The PDF engine runs in a sandbox that does not let it open files. So the app picks the font files,
opens each read-only and hands the engine the open files. The PDF never chooses a file: it can only
say that it needs a font for a script, and the engine answers with the file the app picked for that
script.

## What counts

- **Only the files named below, in the system's own font folders.** Windows: the `Fonts` folder of
  the Windows folder. macOS: `/System/Library/Fonts`, `/System/Library/Fonts/Supplemental` and
  `/Library/Fonts`. Linux: `/usr/share/fonts`. These are the folders that only an administrator can
  write to.
- **Fonts installed for one user only do not count** (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`,
  `~/Library/Fonts`, `~/.local/share/fonts`, `~/.fonts`), and neither does a font with another file
  name, even if it covers the script. Use a font from the lists, installed for all users.
- **The first intact file wins.** A file that is missing, cut short or not a font is passed over for
  the next one in the list. A collection file (`.ttc`) is used from its first font.
- **The lists are read again for each document you open**, so a font installed while the app runs
  is used from the next document you open: close the tab and open the file again (no restart of the
  app is needed). A tab that is already open keeps what it had, and opening the same file again
  only shows that tab.
- If nothing in the list exists for a script, text in that script that the PDF does not carry the
  font for shows as blanks, and the reader tells you once for the document. You can still search,
  select and copy that text.
- **Arabic, Hebrew and Thai** are asked for by the PDF's font name alone, so the reader uses the font
  of the list only for names made for that script (Traditional Arabic, David, Angsana and a few dozen
  more). Text in those scripts that a PDF sets in Arial, Tahoma or Times New Roman without embedding
  the font shows as blanks on every system. Chinese, Japanese and Korean are asked for by character
  set and work whatever the font is called, also when the name is written in the code page of the
  language.
- The reader hands PDFium at most 512 MiB of font data for one document. PDFium keeps one copy of a
  font file for each combination of font name, weight and slant that the PDF asks for, and font
  objects that ask alike share that copy: a PDF merged from many parts that all use "SimSun" costs
  one copy. A file that names hundreds of different fonts gets the first of them drawn and the rest
  blank; the reader tells you. A font file of 55 MB (some of the macOS ones) allows about nine
  different names, one of 19 MB (Noto) about 26.

## The lists

Where a script has several files, they are tried from the top. Checked in CI on the images of the
GitHub runners: Windows and macOS with what the image has, Ubuntu with `fonts-noto-cjk`,
`fonts-noto-core` and `fonts-dejavu-core` installed. The other files have not been run.

### Windows

| Script | Files, in the order they are tried |
|---|---|
| japanese | `YuGothR.ttc`, `YuGothM.ttc`, `msgothic.ttc`, `meiryo.ttc` |
| korean | `malgun.ttf`, `gulim.ttc`, `batang.ttc` |
| chinese-simplified | `msyh.ttc`, `simsun.ttc`, `simhei.ttf`, `Deng.ttf` |
| chinese-traditional | `msjh.ttc`, `mingliu.ttc` |
| arabic | `segoeui.ttf`, `arial.ttf`, `tahoma.ttf` |
| hebrew | `segoeui.ttf`, `arial.ttf`, `david.ttf`, `tahoma.ttf` |
| thai | `leelawui.ttf`, `tahoma.ttf`, `angsa.ttf` |

### macOS

The folders are tried one after the other, and in each folder the files from the top.

| Script | Files, in the order they are tried |
|---|---|
| japanese | `ヒラギノ角ゴシック W3.ttc`, `Hiragino Sans GB.ttc`, `PingFang.ttc`, `Arial Unicode.ttf` |
| korean | `AppleSDGothicNeo.ttc`, `AppleGothic.ttf`, `Arial Unicode.ttf` |
| chinese-simplified | `PingFang.ttc`, `STHeiti Light.ttc`, `Hiragino Sans GB.ttc`, `Arial Unicode.ttf` |
| chinese-traditional | `PingFang.ttc`, `STHeiti Light.ttc`, `Arial Unicode.ttf` |
| arabic | `GeezaPro.ttc`, `Arial Unicode.ttf`, `AlBayan.ttc` |
| hebrew | `ArialHB.ttc`, `Arial Hebrew.ttc`, `Arial Unicode.ttf` |
| thai | `Thonburi.ttc`, `Thonburi.ttf`, `Ayuthaya.ttf`, `Arial Unicode.ttf` |

### Linux

Paths are relative to `/usr/share/fonts`. On Debian and Ubuntu the packages are `fonts-noto-cjk`
(Chinese, Japanese, Korean), `fonts-noto-core` (Arabic, Hebrew, Thai) and `fonts-dejavu-core`
(Arabic and Hebrew).

| Script | Files, in the order they are tried |
|---|---|
| japanese | `opentype/noto/NotoSansCJK-Regular.ttc`, `noto-cjk/NotoSansCJK-Regular.ttc`, `google-noto-cjk/NotoSansCJK-Regular.ttc`, `truetype/noto/NotoSansCJK-Regular.ttc`, `OTF/NotoSansCJK-Regular.ttc`, `truetype/takao-gothic/TakaoPGothic.ttf`, `opentype/ipafont-gothic/ipag.ttf`, `truetype/vlgothic/VL-Gothic-Regular.ttf` |
| korean | `opentype/noto/NotoSansCJK-Regular.ttc`, `noto-cjk/NotoSansCJK-Regular.ttc`, `google-noto-cjk/NotoSansCJK-Regular.ttc`, `truetype/noto/NotoSansCJK-Regular.ttc`, `OTF/NotoSansCJK-Regular.ttc`, `truetype/nanum/NanumGothic.ttf`, `truetype/unfonts-core/UnDotum.ttf` |
| chinese-simplified | `opentype/noto/NotoSansCJK-Regular.ttc`, `noto-cjk/NotoSansCJK-Regular.ttc`, `google-noto-cjk/NotoSansCJK-Regular.ttc`, `truetype/noto/NotoSansCJK-Regular.ttc`, `OTF/NotoSansCJK-Regular.ttc`, `truetype/wqy/wqy-microhei.ttc`, `wqy-microhei/wqy-microhei.ttc`, `truetype/wqy/wqy-zenhei.ttc`, `truetype/arphic/uming.ttc` |
| chinese-traditional | `opentype/noto/NotoSansCJK-Regular.ttc`, `noto-cjk/NotoSansCJK-Regular.ttc`, `google-noto-cjk/NotoSansCJK-Regular.ttc`, `truetype/noto/NotoSansCJK-Regular.ttc`, `OTF/NotoSansCJK-Regular.ttc`, `truetype/wqy/wqy-microhei.ttc`, `wqy-microhei/wqy-microhei.ttc`, `truetype/wqy/wqy-zenhei.ttc`, `truetype/arphic/uming.ttc` |
| arabic | `truetype/noto/NotoSansArabic-Regular.ttf`, `noto/NotoSansArabic-Regular.ttf`, `truetype/dejavu/DejaVuSans.ttf`, `dejavu/DejaVuSans.ttf`, `TTF/DejaVuSans.ttf` |
| hebrew | `truetype/noto/NotoSansHebrew-Regular.ttf`, `noto/NotoSansHebrew-Regular.ttf`, `truetype/dejavu/DejaVuSans.ttf`, `dejavu/DejaVuSans.ttf`, `TTF/DejaVuSans.ttf` |
| thai | `truetype/noto/NotoSansThai-Regular.ttf`, `noto/NotoSansThai-Regular.ttf`, `truetype/tlwg/Garuda.ttf`, `truetype/thai/Garuda.ttf`, `TTF/Garuda.ttf` |
