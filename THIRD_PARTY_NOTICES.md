# Third-party notices

Sift is licensed under GPL-3.0-or-later, with an added permission for Nikon's Image SDK (see [LICENSE](LICENSE) and [NOTICE](NOTICE)). Every component below is compatible with that licence. Sift includes or links the work of others, listed here. Every release also ships `THIRD-PARTY-LICENSES.html` with the full licence text of every Rust crate in the build; regenerate it locally with:

```bash
cargo about generate about.hbs -o THIRD-PARTY-LICENSES.html
```

## Where Sift comes from

Sift started from [Anicrop Studio](https://github.com/R4ksh4/AnicropStudio) by R4ksh4 and its contributors (Apache-2.0), a Windows culling workbench for convention photography. Sift rewrote it in Rust and TypeScript for macOS, Windows, iOS and Android, and distributes the result under GPL-3.0-or-later, as the Apache License permits; the features and approach to culling (eye close-ups, camera AF areas, in-file ratings, finding the RAW for a selection, comparison, guides, parameter frames) come from Anicrop Studio.

## Bundled models and fonts

These files live in `crates/sift-core/assets` and are compiled into the app.

- **YuNet** face detection (`yunet.onnx`): [OpenCV Zoo](https://github.com/opencv/opencv_zoo), MIT
- **YOLOX-Tiny** object detection (`yolox-tiny.onnx`): [Megvii YOLOX](https://github.com/Megvii-BaseDetection/YOLOX), Apache-2.0
- **RTMPose-m AP-10K** animal keypoints (`rtmpose-ap10k.onnx`): [OpenMMLab MMPose](https://github.com/open-mmlab/mmpose), Apache-2.0; trained on [AP-10K](https://github.com/AlexTheBad/AP-10K), CC BY 4.0
- **Inter** typeface (`Inter.ttf`): [The Inter Project Authors](https://github.com/rsms/inter), SIL OFL 1.1 ([text](crates/sift-core/assets/Inter-OFL.txt))

## Rust crates

Most crates are MIT and/or Apache-2.0. Those under other licences:

- **rawler** (RAW decoding, [dnglab](https://github.com/dnglab/dnglab)): LGPL-2.1, which may be combined into a GPL-3.0 program.
- **cssparser**, **selectors**, **dtoa-short**, **option-ext** (pulled in by Tauri): MPL-2.0 (GPL-compatible), used unmodified; their source is on [crates.io](https://crates.io).
- Others under BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0, Unlicense or BlueOak alternatives; see `THIRD-PARTY-LICENSES.html`.

## npm packages shipped in the interface

- react, react-dom, scheduler: MIT, Meta Platforms, Inc. and affiliates
- zustand: MIT, Paul Henschel
- @tauri-apps/api, @tauri-apps/plugin-dialog, @tauri-apps/plugin-opener: Apache-2.0 OR MIT, Tauri Programme within The Commons Conservancy

## Nikon's SDK (official builds only)

Official macOS and Windows builds include Nikon Corporation's NEF/NRW Image SDK and `sift-nikon`, a helper program that drives it. Both are proprietary, are not in this repository and are not covered by the GPL: they are distributed under licence from Nikon and under the terms in [NIKON_SDK_TERMS.md](NIKON_SDK_TERMS.md). NOTICE grants the extra permission (GPL-3.0 section 7) for this combination. The SDK's own third-party notices (Boost, Intel TBB and others) are installed with the app as `Nikon SDK Third Party Legal Notices.rtf`. Builds from this repository alone contain none of this.

## Trademarks

Nikon, NEF, NRW and Picture Control are trademarks of Nikon Corporation. Canon, Sony, Fujifilm, Olympus/OM System, Panasonic, Pentax, Hasselblad, Phase One, Leica and Samsung are trademarks of their respective owners. They are used only to say which files Sift reads; Sift is not affiliated with or endorsed by any of them.
