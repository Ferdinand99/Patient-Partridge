# Changelog

## 0.1.0 (2026-11-01)


### Features

* app skeleton with a working PDF viewer ([78327c7](https://github.com/Ferdinand99/Patient-Partridge/commit/78327c785b80c712183502601932092f0d0751ba))
* engine requests for outline, page text, search and links, and one engine per document ([e4e10f7](https://github.com/Ferdinand99/Patient-Partridge/commit/e4e10f7c84f6d1b3b0669b696befe309c7fdaa61))
* engine requests for outline, page text, search and links, and one engine per document ([7a2f15b](https://github.com/Ferdinand99/Patient-Partridge/commit/7a2f15b05dde41d23c93eb35970ba5248f19778d))
* Hedgerow look, dark mode, tabs, remembered position and file association ([1fbbd28](https://github.com/Ferdinand99/Patient-Partridge/commit/1fbbd28c3934a569afa079959a84c72913185bf6))
* Hedgerow look, dark mode, tabs, remembered position and file association ([03b241b](https://github.com/Ferdinand99/Patient-Partridge/commit/03b241bcfadd219d0ac940b2961fcf6bc49c4359))
* landing page and app icon ([1c4198a](https://github.com/Ferdinand99/Patient-Partridge/commit/1c4198a20f95abce400ddffbdbfcf16b9d714591))
* printing ([f1eee00](https://github.com/Ferdinand99/Patient-Partridge/commit/f1eee00118b1ca6a4dc716b4c19e3f24fa90c007))
* printing ([442ae76](https://github.com/Ferdinand99/Patient-Partridge/commit/442ae76fd2e2d240d9aa916ba1eda5e114da7001))
* reopen the tabs of the last session at start ([62a61df](https://github.com/Ferdinand99/Patient-Partridge/commit/62a61dfffa1eef71b9757c455b3cec39b2d68c2a))
* sandbox the PDF engine ([54572be](https://github.com/Ferdinand99/Patient-Partridge/commit/54572bee67aa29b36091fe54028501486c63b4d3))
* select and copy text, search in the document, and ask before a link is opened ([adfdbba](https://github.com/Ferdinand99/Patient-Partridge/commit/adfdbbab91eee5115bcfbde40575d36b801b0431))
* select and copy text, search in the document, and ask before a link is opened ([49eda46](https://github.com/Ferdinand99/Patient-Partridge/commit/49eda461c2f9926e91c80024b90e94d9d4005edd))
* side panel with page thumbnails and the table of contents ([7ac109c](https://github.com/Ferdinand99/Patient-Partridge/commit/7ac109c170336603656dfa9db233223f844d70d9))
* side panel with page thumbnails and the table of contents ([9bca1c1](https://github.com/Ferdinand99/Patient-Partridge/commit/9bca1c11c46a3588af6a980f8246b895b6de002b))
* update the app from inside, release workflow and macOS signing ([ad4f9c7](https://github.com/Ferdinand99/Patient-Partridge/commit/ad4f9c7f492075e95d799a119aa1df6504752188))


### Bug Fixes

* double and triple click count the presses themselves, because pointer events carry no click count ([3029826](https://github.com/Ferdinand99/Patient-Partridge/commit/302982679428a038cd9eff2063d513453d42df1f))
* find the PDF library inside the app bundle when its path has a symlink on macOS ([f69b10e](https://github.com/Ferdinand99/Patient-Partridge/commit/f69b10eae2f04b5aedd2cf942e245630da495a2b))
* the arrow of the select box is a file of the app, not a data: picture that the content security policy blocks ([b27e927](https://github.com/Ferdinand99/Patient-Partridge/commit/b27e92761fe7935b68beee659269c16e2403a4ad))
* the content security policy allows pictures from the app itself as well as blob: pictures ([91f395d](https://github.com/Ferdinand99/Patient-Partridge/commit/91f395df114512fa46b74acee99a18cc7da39bd3))
* the engine takes its protocol output before PDFium is loaded ([307b77a](https://github.com/Ferdinand99/Patient-Partridge/commit/307b77a15de52ff83da2390388fe37c8f65b06e0))
* the macOS file association check names the PDF type by its UTI only ([2998322](https://github.com/Ferdinand99/Patient-Partridge/commit/299832243bc33ac7df003af7a7ae78411740b32d))
* the smoke test gave the app a HOME that does not exist (macOS startup hang) ([f00267b](https://github.com/Ferdinand99/Patient-Partridge/commit/f00267bfc3f22c23f6745f7078de33e86b278071))
* the Windows association check reads the extension table only, and the smoke test cleans up without failing ([bdd4692](https://github.com/Ferdinand99/Patient-Partridge/commit/bdd4692fe6d13bc1b0d169909d62c66cb7225c48))
* **viewer:** limit the elements drawn over the pages and two smaller reader fixes ([c9dbdec](https://github.com/Ferdinand99/Patient-Partridge/commit/c9dbdec6f3854da5b3432e8a148b1ec0de230bfc))
