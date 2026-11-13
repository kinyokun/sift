# README 用图

主页（[README.md](../../README.md)、[README.en.md](../../README.en.md)）上的截图、动图和图表都在这个目录里，每张都有中文（`-zh`）和英文（`-en`）两版。它们是用真实的应用界面生成的：开发桥接（`examples/bridge`）提供真实的命令，无头 Chrome 打开开发版界面，按脚本操作后截图或录屏。照片全部是公开授权的样张，来源见下方 [照片来源](#照片来源)。

*Everything here is generated from the real UI in headless Chrome; the photos are public CC0 samples, credited below.*

## 重新生成

准备一个样张文件夹（下文叫 `Demo`），放进下方列出的照片，再准备几个 `.cube` / `.NP3` 滤镜。然后：

```bash
# 1. 启动桥接（release 构建，数字才有参考价值）和界面
SIFT_NIKON_ROOT=<尼康引擎所在的 .app 或目录> cargo run --release -p sift --example bridge &
pnpm dev &

# 2. 截图和录屏，中英文各一遍（输出到 /tmp/sift-readme/{zh,en}）
LOOKS="$(ls -d "$PWD"/looks/* | paste -sd, -)" node scripts/readme-shots.mjs /path/to/Demo /tmp/sift-readme
LANG_UI=en LOOKS="$(ls -d "$PWD"/looks/* | paste -sd, -)" node scripts/readme-shots.mjs /path/to/Demo /tmp/sift-readme

# 3. 录屏转成动图
for l in zh en; do for n in flip zoom; do
  node scripts/readme-anim.mjs /tmp/sift-readme/$l/$n docs/readme/$n-$l.webp
done; done

# 4. 加窗口框、拼主图、画性能图和社交预览图
node scripts/readme-art.mjs /tmp/sift-readme docs/readme docs/readme/perf.json
```

- `ONLY=library,viewer` 只跑其中几段；段名有 `library`、`viewer`、`eyes`、`looks`、`stack`、`clipping`、`export`、`light`、`phone`、`flip`、`zoom`、`perf`。`readme-art.mjs` 的 `ONLY` 按输出文件名前缀过滤
- `perf` 一段不出图，在终端打印界面里实测的翻页耗时和第一屏耗时；要测近千张的文件夹，可以用硬链接把样张复制几十份
- 核心的耗时（取内嵌预览、生成缩略图、完整解码）用 `cargo run --release -p sift-core --example probe -- --preview <输出目录> <文件…>` 和 `--develop` 测；`perf.json` 是性能图上的数字，换机器重测后改这里
- 没有尼康引擎也能跑，只是 NP3 滤镜换成近似还原
- GitHub 仓库设置里的 Social preview 不能用 WebP，上传前转一下：`dwebp docs/readme/social-preview.webp -o social-preview.png`
- 需要 Chrome、`cwebp` / `dwebp`（libwebp）和带 libwebp 的 `ffmpeg`

## 照片来源

### Wikimedia Commons（CC0）

动物照片都来自 Wikimedia Commons，作者以 [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) 放弃了全部权利。截图里用的文件名是重新起的；GPS 信息已删除，个别照片缩小到长边 6000 px。

| 截图里的文件 | 原图 | 作者 |
| --- | --- | --- |
| `cat-portrait.jpg` | [Domestic shorthair cat portrait in grass](https://commons.wikimedia.org/wiki/File:Domestic_shorthair_cat_portrait_in_grass.jpg) | Marko Milivojevic（经 pixnio） |
| `tabby-on-the-porch.jpg` | [Young tabby cat keeping watch](https://commons.wikimedia.org/wiki/File:Young_tabby_cat_keeping_watch.jpg) | W.carter |
| `cat-in-the-sun.jpg` | [Cat Bathing in Sunbeam](https://commons.wikimedia.org/wiki/File:Cat_Bathing_in_Sunbeam.jpg) | photos-public-domain.com |
| `boxer.jpg` | [Boxer dog in iran](https://commons.wikimedia.org/wiki/File:Boxer_dog_in_iran.jpg) | Mostafa Meraji |
| `dog-on-the-grass.jpg` | [Dog resting on the grass](https://commons.wikimedia.org/wiki/File:Dog_resting_on_the_grass.jpg) | Jose Lodos Benavente |
| `kingfisher-on-a-branch.jpg` | [Eisvogel kingfisher](https://commons.wikimedia.org/wiki/File:Eisvogel_kingfisher.jpg) | Frank-2.0 |
| `kingfisher-on-a-rock.jpg` | [00A17 common kingfisher Adatt Kole](https://commons.wikimedia.org/wiki/File:00A17_common_kingfisher_Adatt_Kole.jpg) | Drsssuresh1961 |
| `white-throated-kingfisher.jpg` | [White-throated kingfisher image 6](https://commons.wikimedia.org/wiki/File:White-throated_kingfisher_image_6.jpg) | Drsssuresh1961 |
| `pied-kingfisher-on-a-twig.jpg` | [Pied kingfisher David Raju](https://commons.wikimedia.org/wiki/File:Pied_kingfisher_David_Raju.jpg) | Davidvraju |
| `pied-kingfisher-01.jpg` … `07.jpg` | [Pied kingfisher at madayipara](https://commons.wikimedia.org/wiki/File:Pied_kingfisher_at_madayipara.jpg) 一组 | Afsarnayakkan |
| `pony.jpg` | [A domestic horse in Bemmel, Netherlands](https://commons.wikimedia.org/wiki/File:A_domestic_horse_in_Bemmel,_Netherlands.jpg) | Cultureel Gelderland |
| `horse.jpg` | [Horse posing in the grass](https://commons.wikimedia.org/wiki/File:Horse_posing_in_the_grass.jpg) | Jose Lodos Benavente |
| `cross-fox.jpg` | [Cross fox in Lewis 2](https://commons.wikimedia.org/wiki/File:Cross_fox_in_Lewis_2.jpg) | Yuan Zhuang |
| `squirrel.jpg` | [Squirrel on a Branch](https://commons.wikimedia.org/wiki/File:Squirrel_on_a_Branch.jpg) | Tfbybyhf |
| `black-squirrel.jpg` | [Sciurus carolinensis (black morph) on a rock](https://commons.wikimedia.org/wiki/File:Sciurus_carolinensis_(black_morph)_on_a_rock,_Jardin_universitaire_Roger-Van_den_Hende,_Quebec_City.jpg) | Wilfredor |

### raw.pixls.us（CC0）

RAW 样张来自 [raw.pixls.us](https://raw.pixls.us)，上传者同意以 CC0 公开。文件名保留原样，拍摄者按文件里记录的署名列出。

| 文件 | 机身 | 拍摄者 |
| --- | --- | --- |
| `CZP_0299.NEF` | Nikon Z 6II | Christopher Cameron |
| `DSC_0040.NEF` | Nikon Z f | Jonathan Hrovath |
| `DSC_0750.NEF` | Nikon Z 6 | Ludger Heide |
| `DSC0018.nef`、`DSC0070.nef` | Nikon Z fc | I.S. |
| `Z72_0027.NEF` | Nikon Z 7II | 未署名 |
| `Z8A_0412.NEF` | Nikon Z 8 | 未署名 |
| `Z9_1187.NEF` | Nikon Z 9 | G Edward Johnson |
| `DSC02429.ARW` | Sony α7C II | 未署名 |
| `DSC03318.ARW` | Sony α7 III | 未署名 |
| `IMG_0729.CR2` | Canon EOS 5D Mark IV | Lothar Kimmeringer |
| `R5A_2034.CR3` | Canon EOS R5 | Benjamin Grimm-Lebsanft |

滤镜对比墙里的滤镜是为截图临时做的，没有放进仓库；换成任意 `.cube`、`.3dl`、HaldCLUT `.png` 或 `.NP3` 都可以。
