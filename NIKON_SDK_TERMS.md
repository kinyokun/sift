# Nikon NEF/NRW Image SDK – terms of use

Official macOS and Windows builds of Sift include Nikon Corporation's
NEF/NRW Image SDK ("the Nikon SDK"): Nikon's libraries and data files, and a
small helper program (`sift-nikon`) that Sift starts to drive them. Sift uses
them to develop Nikon RAW files exactly as the camera would.

The Nikon SDK is **not** open source and is **not** covered by Sift's
GPL-3.0-or-later licence. It is distributed with Sift under a licence from
Nikon Corporation, and the GPL's section 7 additional permission in
[NOTICE](NOTICE) allows this combination. By using a build of Sift that
includes the Nikon SDK you agree to the following:

1. **Use.** You may use the Nikon SDK only as part of Sift, to view and
   develop images on your own devices. You may not use it separately from
   Sift or build it into other software; to do that, obtain it from Nikon
   under Nikon's own agreement (<https://sdk.nikonimaging.com/apply/>).
2. **Redistribution.** You may pass on Sift's official installers unchanged
   and under these same terms. You may not otherwise distribute, rent, lease,
   sublicense or sell the Nikon SDK or any part of it.
3. **No modification or reverse engineering.** You may not modify,
   disassemble, decompile or otherwise reverse engineer the Nikon SDK or the
   helper program, except where the law gives you that right regardless of
   this term.
4. **Ownership and notices.** The Nikon SDK and all rights in it belong to
   Nikon Corporation and its licensors. Do not remove or alter any copyright,
   trademark or other notice in it. The third-party notices that come with
   the Nikon SDK are installed with Sift
   (`Nikon SDK Third Party Legal Notices.rtf`, in the application's
   resources).
5. **No warranty.** The Nikon SDK is provided "as is", without warranty of
   any kind, express or implied, including the implied warranties of
   merchantability, fitness for a particular purpose and non-infringement.
6. **No liability.** To the extent the law allows, neither Nikon Corporation
   nor the Sift authors are liable for any damages arising from the use of,
   or the inability to use, the Nikon SDK, including lost profits, lost data
   and indirect or consequential damages.
7. **Nikon as beneficiary.** Nikon Corporation is a third-party beneficiary
   of these terms and may enforce them directly.
8. **Ending.** Your right to use the Nikon SDK ends if you break these
   terms. Sift keeps working without it: remove the build that includes the
   Nikon SDK and build Sift from source instead.

Builds made from this repository alone contain nothing of the Nikon SDK;
these terms do not apply to them.

Nikon, NEF, NRW and Picture Control are trademarks of Nikon Corporation. Sift
is not affiliated with or endorsed by Nikon.

---

## 简体中文说明（以英文为准）

Sift 官方发布的 macOS 与 Windows 安装包内置了尼康公司的 NEF/NRW Image SDK（尼康的库与数据文件，以及驱动它们的小程序 `sift-nikon`），用来按机内效果显影尼康 RAW。

这部分不是开源软件，不适用 Sift 的 GPL-3.0-or-later 许可，而是经尼康授权随 Sift 分发。使用内置了它的 Sift 即表示你同意：

1. 只能把它作为 Sift 的一部分使用，不得单独使用或用于其他软件
2. 可以原样转发官方安装包，除此之外不得分发、出租、转授权或出售它
3. 不得修改、反汇编、反编译或以其他方式对它进行逆向工程（法律强制允许的情形除外）
4. 它的全部权利归尼康公司及其许可方所有，不得去除或改动其中的版权与商标声明
5. 它按“现状”提供，不附带任何明示或默示的保证
6. 在法律允许的范围内，尼康公司与 Sift 作者不对因使用或无法使用它而产生的任何损失负责
7. 尼康公司是本条款的第三方受益人，可以直接主张本条款
8. 违反本条款时，你使用它的权利即告终止

仅由本仓库源码构建的 Sift 不含任何尼康 SDK 的内容，不受本条款约束。
