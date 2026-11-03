# Features in detail

This page describes what Patient Partridge does today, in the detail the [README](../README.md) leaves out. What it cannot do, and where the edges are, is in [known-limits.md](known-limits.md); how the app is protected is in [security.md](security.md); how it is put together is in [architecture.md](architecture.md).

- [Opening files and tabs](#opening-files-and-tabs)
- [Reading](#reading)
- [Side panel](#side-panel)
- [Selecting and copying text](#selecting-and-copying-text)
- [Search](#search)
- [Links and attachments](#links-and-attachments)
- [Printing](#printing)
- [Zoom and moving around](#zoom-and-moving-around)
- [Remembering your place and your tabs](#remembering-your-place-and-your-tabs)
- [Look](#look)
- [Passwords and damaged files](#passwords-and-damaged-files)
- [PDF viewer registration](#pdf-viewer-registration)
- [Saving](#saving)
- [Unsaved changes and undo](#unsaved-changes-and-undo)
- [Annotations](#annotations)
- [Filling in forms](#filling-in-forms)
- [Page tools](#page-tools)
- [Making forms](#making-forms)
- [A picture of a signature](#a-picture-of-a-signature)
- [Digital signatures, read-only](#digital-signatures-read-only)
- [Updates](#updates)

## Opening files and tabs

Open one or more PDFs from the Open button (Ctrl+O), by dropping files on the window, from the command line, or with "Open with" from a file manager. Each file opens in its own tab (Ctrl+W closes a tab, Ctrl+Tab and Ctrl+Shift+Tab move between tabs). Opening a file that is already open shows its tab.

## Reading

Read in one continuous scrolling view. Only the pages you can see (and a few around them) are drawn, so opening is quick and memory stays flat as documents get longer. A tab you are not looking at gives up its drawn pages.

## Side panel

A side panel (Ctrl+B) shows page thumbnails and the table of contents of the PDF (its bookmarks). Thumbnails are drawn only for the part of the list you can see, and only after the page you asked for is on screen, so the number of pages does not slow down opening (measured with a 500-page file; larger ones have not been timed). Entries of the contents that point to a web address or a file are shown but never followed.

## Selecting and copying text

Select text with the mouse (double click for a word, triple click for a line, Ctrl+A for everything) and copy it with Ctrl+C, the system's Copy, or the Copy entry of the right mouse button. The text and the place of every character come from PDFium through the engine; nothing is read from the file by the window.

## Search

Search in the document (Ctrl+F). The matches are highlighted on the pages, Enter and Shift+Enter (or F3) move between them, and the count is shown. Search ignores upper and lower case and line breaks, and runs page by page in the background, so you can stop it at any time. Words that a PDF writes with a ligature such as "fi" in one character may not be found.

## Links and attachments

Links inside a PDF are never opened by themselves. Pressing one shows the whole address and asks first. Only web addresses (http and https) can be opened, in your normal browser; anything else (a file, a program, a script, a mail address) is refused with an explanation. Links to another page of the same document just go there. Attachments in a PDF are never opened or even listed.

## Printing

Print (Ctrl+P): all pages, the page in view, or a list such as 1-3, 7, 9-. The pages are drawn by the engine at 150 dpi, packed as JPEG pictures one at a time (a few hundred kilobytes each) and then the print dialog of your system opens; nothing is sent anywhere else. A printed page is a picture of the page, not text. This is covered by unit tests with a fake printer and by a browser test that checks what is shown in print media; it has not been tried on a real printer on any system yet.

## Zoom and moving around

Zoom in and out, or fit the page to the window width. Ctrl with the mouse wheel zooms too.

Move with the mouse wheel, the arrow keys, Page Up and Page Down, Space, Home and End, or jump to a page number (Ctrl+G).

## Remembering your place and your tabs

Patient Partridge remembers where you stopped in each file and puts you back there when you open it again. The place (page, how far down, zoom) is kept in a small file on your computer, together with the path of the file, for the last 500 files. Nothing is sent anywhere. "Forget remembered positions" in the settings menu (the three dots) empties it.

The tabs you had open come back the next time you start the app, in the same order with the same tab in front, each at its remembered place. Only the file paths are kept, in the same folder as the positions. A file that has gone is skipped with one short notice, and a password-protected file stays closed until you open it. "Reopen tabs at start" in the settings menu turns this off; that, and "Forget remembered positions", delete the list.

## Look

The look follows the system's light or dark setting. The settings menu has Auto, Light and Dark. The pages themselves stay white, like paper.

## Passwords and damaged files

Password-protected files ask for the password. A damaged file, or a file that is not a PDF, gives a clear message instead of a crash.

## PDF viewer registration

The installers register Patient Partridge as a viewer for PDF files, so it appears under "Open with". It does not take over as the default reader by itself.

## Saving

Annotations are the first thing that changes a document, and they are saved the way every change will be. The Save and Save a copy buttons in the toolbar (or Ctrl+S and Ctrl+Shift+S) do the work: Save writes a document that has changes, and Save a copy writes the open document to a new file that you choose, and the tab then follows that file. Save is greyed out while there is nothing to save.

How a save works, whatever the document: the engine, which still writes nothing, has PDFium put the whole document into memory and hands the bytes to the app piece by piece; the app writes them to a new file in the same folder and flushes it to the disk; a second, sandboxed engine opens that new file and checks it (it opens with the same password, has the same number of pages, is protected the same way, holds the same signatures); and only then does the new file replace the old one in a single step. If anything goes wrong on the way (a full disk, a crash, a file that does not pass the check) the file on disk is exactly as it was.

A read-only file is not replaced, a link is followed, and the file keeps its permissions. If something else changed the file after you opened it, you are asked first: replace it, save a copy, or cancel. A password-protected document stays protected with the same password, or the save is refused with a message.

A document with a digital signature is not saved without a warning (see [Known limits](known-limits.md#saving-and-digital-signatures)): the app counts a document as signed when PDFium finds a signature in the field list or in the widgets of any page, and also when the bytes of the file mention a signature (`/ByteRange`, `/DocMDP`, `/SigFlags`), so a warning that was not needed is possible and a missed one is not.

The limits of saving (what the check can and cannot catch, size and the file system, a file that changes while it is open) are in [known-limits.md](known-limits.md#a-file-that-changes-while-it-is-open).

## Unsaved changes and undo

A tab with changes that are not saved shows a dot, and the window title starts with a dot. Closing the tab, closing the window or quitting asks first (Save, Don't save, Cancel) and holds the request back until you answer. An update waits for you too: it is downloaded, but it is not installed while a document has changes that are not saved; you can save and install in one step, or install later from the toolbar. The app itself refuses to install while any document has unsaved changes. Undo and redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z) take back and make again the changes of the open document, one by one, up to the last 200. Page changes, annotations and form fields share this one history and are taken back newest first, whichever kind they are: taking back a delete, a move or added pages puts the copy of the document back with the annotations and field values it had, and the annotations made before the change can then be taken back too. When the copy that takes a page change back is gone, Undo says so, and says how many changes before it in the history cannot be reached either.

## Annotations

**Annotations (version 0.2).** The Annotate button (or Ctrl+Shift+A) opens a small bar with the tools: Select, Highlight, Underline, Strike out, Note, Pen and Stamp, six colours, Delete, Undo and Redo. Highlight, Underline and Strike out mark the text you select (select first and then press the tool, or press the tool and then select), also right-to-left text and text on turned pages: the engine finds the boxes of the characters itself. A Note is placed with a click and opens a box for its text. The Pen draws with the mouse, a pen or a finger, in a thin, medium or thick line, and draws the same at every zoom and on a page that is turned. A Stamp (Approved, Rejected, Draft, Final, Confidential, Reviewed) is chosen from a menu and placed with a click. Select picks an annotation: Enter changes its text, the arrow keys (with Shift for bigger steps) move a note, and Delete takes it off.

Annotations that a file already has are shown in the same way and can be changed or removed, with some limits (see [Known limits](known-limits.md#annotations)). An annotation the file marks read-only or locked, and every annotation of a document whose permissions forbid annotating, cannot be changed, and the bar says so.

They are real PDF annotations written by PDFium with appearance streams, not marks drawn over the page, so other readers can show and edit them (the tests read the files with `qpdf` and Poppler; other readers were not tried), and are saved by the [safe save](#saving) above. With the bar closed the pages read as they do without annotations: the boxes over highlights and notes take no clicks, so links and text selection work, nothing stays picked, and the arrow keys, Enter and Delete act on the page as they do in any document.

A change that was made can be undone and done again until the document is closed, also after a save (deleting an annotation that came with the file is the one change Undo cannot take back, and it clears Redo; taking back a delete, a move or added pages made before it loads the document as it was then, with that annotation in it, and taking back a turn or a crop does not). A password-protected document stays protected, and a signed document gets the warning described under [Known limits](known-limits.md#saving-and-digital-signatures) before an annotation is saved into it.

The text of a note or a comment, and the author, come from the file: they are shown as text only and cut to fixed lengths (4 KiB for a text, 256 bytes for an author, 500 annotations on a page, 256 strokes and 20,000 points in a drawing). A second program (`qpdf` for the structure, Poppler for drawing) reads the files in the tests.

## Filling in forms

A PDF form (an AcroForm) can be filled in and saved, so that other PDF readers and the tools that read form values show the result. Text fields (one line or several), check boxes, radio buttons, drop-down lists (with or without a box to type in) and list boxes (one choice or several) are supported.

Each field is a control placed over its picture on the page, so it follows the zoom and a turned page, takes the keyboard (Tab and Shift+Tab walk the fields in page order, Space toggles a check box, the arrow keys move between the radio buttons of a group, Enter ends a line of text and Escape puts the old text back) and has a name for a screen reader (the tooltip of the field, or else its name). A required field that is still empty is outlined with a dashed line, and a field marked read only cannot be changed.

A change is sent to the engine when you leave the field; PDFium writes the value together with the picture that other readers draw (the appearance of the field), the page is drawn again, the tab shows the dot, and Ctrl+Z and Ctrl+Y take the change back and make it again. Saving uses the [safe save](#saving) described above, including the question before a signed document is rewritten.

Text that is too long for the document (more than 8 KiB in a field) is not given up: it stays in its box, which turns white with a red outline and shows the text, and the document counts as changed. Save, Save a copy and Save and quit refuse while it is there; the message names the field and the page and has a button that takes you to it. Shorten the text, or press Escape in the box to put back what the document has.

The text a form carries (its field names, tooltips, values and the choices of a list) comes from the file and is shown as text and nothing else; it is capped in the protocol (names 512 bytes, values 8 KiB, 500 choices a list, 1,000 fields a page).

What forms cannot do (XFA forms, PDF JavaScript, actions that change a form) is in [known-limits.md](known-limits.md#forms).

## Page tools

The Pages panel of the side panel (Ctrl+B) is also where pages are worked on. Pick pages with a click (Ctrl adds or removes a page, Shift picks a run) or with the keyboard (the arrow keys, Home, End and Page Up and Down move, Shift extends, Space picks the page you are on, Ctrl+A picks all, Escape lets go). The bar above the thumbnails then turns the picked pages left or right (the keys Shift+R and R), deletes them (Delete), crops them, copies them to a new file, adds a blank page after them, or adds all the pages of other PDF files after them. Adding files is also how files are merged: pick several in the dialog and their pages are added in the order the system gives them. Drag the picked pages to move them, or press Alt with the Up or Down arrow. Every change can be taken back and made again with Ctrl+Z and Ctrl+Y, and is written by the ordinary save (Ctrl+S: a signed document asks first, as always). Nothing else writes a file, except the copy of some pages to a new file, which goes through the same steps as a save: a new file next to the target, flushed, read back, opened by a second sandboxed engine that must find the pages the first said it made, and only then put in place. A tool that takes a moment shows a box with a bar. Only adding pages from other files and copying pages out have a Stop button there; a change to the document itself cannot be stopped half way, and its box says so. What each tool keeps and loses is under Known limits.

## Making forms

The Edit form button (next to Annotate) opens a bar with the tools Text, Check box, Radio, Drop-down, List and Signature, and a panel for the picked field. Click on a page to place a field of the usual size, or drag to give it your own; the new field is picked and named (Text 1, Check box 1 and so on). A picked field has eight handles to resize it and moves by dragging; the arrow keys move it by a point (ten with Shift), Alt with the arrow keys resizes it, and Delete takes it away. A box snaps to the edges of the page, the edges of the other fields and the ends of the words and the lines of the printed text when it comes within six pixels; Alt turns that off. The radio tool adds each click to one group until Escape. The panel has the name (every field in the document needs its own, and a dot is not allowed because a dot makes a field part of another field), the tooltip, required and read only, and for each kind: several lines, a password, one box for each character, the longest text, the size of the text and the starting text of a text field; the value and the starting state of a check box and of each radio button; the choices and the starting choice of a drop-down and a list (a drop-down may let the reader type another value, a list may take several choices). It also shows the fields of the page in the order Tab walks them, with buttons to move one earlier or later; that order is the order of the annotations of the page. Changes rest for a moment and are sent as one batch, and before a save, a print, a page tool or a step back; a batch is one step in the history, so Ctrl+Z and Ctrl+Y take it back and make it again, and the document counts as changed. The fields are real form fields with their own pictures for each state, written after the bytes PDFium saved and loaded again; the safe save writes them like everything else, and the tests read the files with `qpdf`, Poppler and `pdftk`, fill them in with `pdftk` and with the app's own form code, and fuzz the writer. A field made here never has an action or a script, and the window has no way to ask for one. The Edit form button and the annotation bar are not open together.

## A picture of a signature

The Signature picture tool of the annotation bar (key G) puts a picture of your signature on a page. **It is a picture, not a digital signature: it does not show who placed it, anyone can copy it, and Patient Partridge makes no claim about what any person or program will make of it.** You make the picture in one of three ways: draw it with the mouse, a pen or a finger (the lines are kept as lines); type your name in one of three script fonts (the letters are written into the file as curves, so the file carries no font); or import an image of your signature (the window reads the file, can make its white background see-through, and sends only the pixels, so the PDF engine needs no image reader and no file leaves the window). A picture can be saved on this computer for later, and used again from the Saved tab, renamed or deleted; "Forget saved signatures" in the settings menu deletes them all. The saved pictures are in one small file in the app's local data folder (on Windows that is Local AppData, which does not roam with a profile and is not redirected to a server; a file that an earlier version kept in Roaming AppData is moved there at the start, tried again at the next start if the move did not work, and removed from Roaming only once all of its pictures are in the new file (those the new file lacks are added to it; a file that cannot be read stays where it is); "Forget saved signatures" deletes it too), kept much the way the remembered positions are (private to your user on Linux and macOS, checked again when read, written and flushed before it replaces the old file, never sent anywhere): anyone who can read that folder can use them. If another program changes the file while the app runs, the app reads it again before its next change. A file that cannot be read as a list (damaged, cut short, from another version) is never written over: the first change moves it aside as `signatures.json.bad` (the next free `.bad.1` to `.bad.9` when there is one already, so an earlier one is never written over) before it writes the new list, "Forget saved signatures" deletes those files too, and when it cannot be moved aside nothing is written. Click on a page to place the picture, drag it to move it, drag its corner (or press plus and minus) to make it bigger or smaller with its proportions kept, and undo and redo each step. It is written as a stamp annotation that is marked to print, so other programs print it too. It is one more change to the document like the other annotations: the check of the digital signatures drops its result for the tab, and it waits while the pages are being changed. See "Known limits".

## Signing with a certificate

The Sign with a certificate entry of the settings menu (the three dots) adds a digital signature made with the key in a certificate file (.p12 or .pfx) that you choose. You type the password of the file, which opens it and is not saved. The box shows who the certificate names, who issued it, the key and the period it is in use, and says in one line each when the certificate was made by its owner, has expired, is not in use yet or says it is not meant for signing; none of these stops you. You then choose where the signature shows (a box you drag on a preview of a page, a signature field the file already has, or no box at all), what it says (a name, a reason, a place) and, if you like, a picture of your signature made the way the Signature picture tool makes one. The signed file is a new file that you name, and the document you had open is not changed. It is opened in a tab of its own with editing off: any change would break the signature, so to change it you save a new copy. A document with changes that are not saved is saved first, so that what is signed is a file you have. RSA keys of 2048 to 4096 bits and ECDSA keys on the curves P-256 and P-384 are signed (other kinds of key are refused with their name). The signing time is the clock of this computer, and nobody else has confirmed it. The signature is a CAdES detached signature with the certificate chain of the file and the signing-certificate attribute; no time stamp is added. What other programs make of the signature is up to them: Patient Partridge says what it did and nothing about whether anyone will accept it. See [docs/certificate-signing.md](docs/certificate-signing.md).

## Digital signatures, read-only

A document with digital signatures gets a calm badge in the toolbar, and a panel at the right with the details. The engine reads the signatures through PDFium and checks them in its sandbox; the window only shows what it is told. Each signature is in one of five states, always in these words: **Signature intact; signer trusted**, **Signature intact; signer not verified**, **Changed after signing**, **Signature broken**, or **Signature not checked**. The panel shows the certificate of the signer as text (name, organisation, issuer, dates, key, serial number, fingerprint) and, apart from that and marked as not checked, what the file says about the time, the reason and the place. A picture of a signature on a page says nothing; only the panel reports on a signature. See [Digital signatures: what is checked](digital-signatures.md)..

## Updates

Patient Partridge looks for a newer version when it starts and every 6 hours (a small pill appears in the toolbar), and **Check for Updates…** in the settings menu does it on request. The release notes are shown first; nothing is downloaded until you press Download and Install. The update is checked against the project's signing key before anything is installed, the PDF engines are stopped for the install, and the files you had open come back after the restart. The automatic check is on by default and can be turned off in the same menu. The Windows installer, the macOS app and the Linux AppImage update this way; a Linux deb package cannot, and shows a notice with a link to the releases page instead. How it works, what is sent and what has been tried: [updates.md](updates.md).
