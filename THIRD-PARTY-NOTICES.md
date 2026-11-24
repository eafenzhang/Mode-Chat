# 第三方组件与许可

本文件列出 `Mode Chat` 仓库直接包含的第三方资源。Rust 依赖的许可由
`Cargo.lock` 与各 crate 自带声明约束，此处不逐条罗列。

## 图标

| 用途 | 来源 | 许可 | 许可全文 |
| --- | --- | --- | --- |
| 通用 UI（侧栏、菜单、按钮） | [Boxicons](https://boxicons.com) | MIT | [LICENSE-boxicons.txt](assets/icons/LICENSE-boxicons.txt) |
| 连接器品牌标 | [simple-icons](https://simpleicons.org) / [mdi](https://materialdesignicons.com) | CC0-1.0 / Apache-2.0 | 资产内嵌，见下 |
| IM 频道手机端标 | [TDesign](https://tdesign.tencent.com) | MIT | 资产内嵌，见下 |
| 模型厂商标 | [LobeHub Icons](https://github.com/lobehub/lobe-icons) | MIT | [LICENSE-lobe.txt](assets/icons/LICENSE-lobe.txt) |
| 连接器（部分） | [Iconify logos](https://icon-sets.iconify.design/logos/) | CC0-1.0 | [LICENSE-logos.txt](assets/icons/LICENSE-logos.txt) |

说明：

- 连接器图标统一压成单色 `currentColor`，运行时按品牌色染色。
- 模型厂商图标为多色矢量，通过 `img()` 渲染（GPUI 的 `svg()` 会拍平成单色）。
- `LICENSE-lucide.txt` 保留自早期版本（曾短期使用 Lucide），当前未引用其资产。

## 字体

| 字体 | 许可 | 许可全文 |
| --- | --- | --- |
| [Geist](https://vercel.com/font) | SIL Open Font License 1.1 | [Geist-LICENSE.txt](assets/fonts/Geist-LICENSE.txt) |

中文界面字体使用系统自带的「Microsoft YaHei UI」，不随包分发。

## 上游代码

本仓库是 [zai-org/ZCode](https://github.com/zai-org/ZCode) 的衍生物，上游部分按
Apache-2.0 授权（全文见 [LICENSE-APACHE](LICENSE-APACHE)）。本仓库自身代码按
MIT 授权（见 [LICENSE](LICENSE)）。变更说明见 [NOTICE.md](NOTICE.md) 第 4 节。

## 应用图标

`assets/icons/app.ico` 与 `app.png` 为本项目自制，随 MIT 发布。
