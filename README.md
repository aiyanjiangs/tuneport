# TunePort

[![CI](https://github.com/aiyanjiangs/tuneport/actions/workflows/ci.yml/badge.svg)](https://github.com/aiyanjiangs/tuneport/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/aiyanjiangs/tuneport?label=release)](https://github.com/aiyanjiangs/tuneport/releases/latest)
[![License](https://img.shields.io/github/license/aiyanjiangs/tuneport)](LICENSE)

一个 Windows 桌面的**本地音频格式转换工具**：把音乐客户端下载到本地的音频文件，还原成原始的 MP3 / FLAC / OGG，并保证标签、歌词、文件名都完整。

全程离线：不联网、不上传、不下载任何内容、不改动源文件。

> **免责声明**：本项目是一个**格式转换技术实现**，用于把你已经合法获得的本地文件转成通用格式（例如在自己的播放器里播放、做个人备份）。
> 它**不提供任何音乐内容的下载、搜索或分享能力**，也**不提供绕过付费会员的服务**。
> 请仅对你拥有合法权利的文件使用本工具，不要传播转换后的作品。使用者自行承担由此产生的法律责任。
> 如果你不确定自己的使用场景是否合规，请不要使用。

---

## 支持格式

| 来源 | 扩展名 |
|---|---|
| 网易云音乐 | `.ncm` |
| QQ 音乐（旧） | `.qmc0` `.qmc2` `.qmc3` `.qmcflac` `.qmcogg` `.qmc` |
| QQ 音乐（新） | `.mflac` `.mflac0` `.mgg` `.mgg0` `.mgg1` `.mggl` |
| 酷我音乐 | `.kwm` |

输出格式由文件内容决定（看解密后的文件头），**不做转码**：源是 FLAC 就出 FLAC。

## 功能

| 能力 | 说明 |
|---|---|
| 批量输入 | 拖入文件或整个文件夹（递归扫描） |
| 流式处理 | ncm 按 1 MB 分块边解边写，内存不随文件变大 |
| 写标签 | ncm：MP3 写 ID3v2.3 / FLAC 写 Vorbis Comment。其余格式自带标签，不重写 |
| 读标签 | FLAC Vorbis Comment / MP3 ID3v2 / Ogg Vorbis 与 Opus 头 |
| 命名 | 默认「歌手 - 歌名」，清洗 Windows 非法字符，重名自动加 ` (2)` |
| 歌词 | 可选把同名 `.lrc` 一起复制，并跟随新文件名 |
| 试听 | 转换完直接播放，不落地临时文件 |
| 失败处理 | 单文件失败不影响整批；标红置顶，可单独重试 |
| 断点续跑 | 列表和每个文件的状态存盘，重开程序自动接上 |
| 设置 | 命名模板、同名策略、输出目录、并发数、完成后打开目录、默认复制歌词 |

## 界面

![界面](docs/toolbar-switch.png)

## 使用

前往 **[Releases](https://github.com/aiyanjiangs/tuneport/releases/latest)** 下载：

| 文件 | 说明 |
|---|---|
| [TunePort-1.0.0-x64-setup.exe](https://github.com/aiyanjiangs/tuneport/releases/latest/download/TunePort-1.0.0-x64-setup.exe) | 安装版，装到用户目录，**不需要管理员权限** |
| [TunePort-1.0.0-portable.zip](https://github.com/aiyanjiangs/tuneport/releases/latest/download/TunePort-1.0.0-portable.zip) | 绿色版，解压即用，不写注册表 |

> 上表用的是固定版本号。新版本发布后如果没更新这里，以 Releases 页面上的文件名为准。

系统要求：64 位 Windows 10 / 11。唯一依赖是系统自带的 **WebView2**（Win10 1803+ / Win11 均自带）。

首次运行若被 SmartScreen 拦下，点「更多信息 → 仍要运行」（exe 未做代码签名，见 [FAQ](docs/FAQ.md)）。

## 从源码构建

需要 Rust 1.77+、Node 18+、WebView2。

```bash
git clone https://github.com/aiyanjiangs/tuneport.git
cd tuneport
npm install

npm test            # 界面测试（jsdom）
npm run build       # 编译 + 打 NSIS 安装包
```

产物：

```
src-tauri/target/release/tuneport.exe                       # 绿色版
src-tauri/target/release/bundle/nsis/*.exe                  # 安装包
```

只编译核心库：

```bash
cd core-rs && cargo test
```

## 测试

```bash
cd core-rs && cargo test       # 41 项:格式解析、密文算法、标签、端到端
cd src-tauri && cargo test     # 6 项:设置读写、队列读写、BOM 兼容
npm test                       # 73 项:界面逻辑与 IPC 适配层
```

其中格式算法的关键路径用**公开的测试向量**校验，而不是只做自己的往返测试：

- QMC1 掩码表逐字节
- QMC2 映射密码 / 改版 RC4 的已知输出
- ekey 的裸密钥与 EncV1 两条解析路径
- 三种文件尾部（V1 / QTag / musicex）识别
- 酷我：静音段反推密钥、中段静音、非 kwm 拒绝

## 项目结构

```
core-rs/                 Rust 核心(可独立 cargo test)
  src/ncm.rs             网易云:容器解析 + 流式解密
  src/qmc.rs             QQ 音乐:QMC1 / QMC2 + 版权信息读取
  src/kwm.rs             酷我:密钥恢复 + 解密
  src/tags.rs            ID3v2.3 / Vorbis Comment 写入
  src/naming.rs          文件名清洗与重名
  src/engine.rs          批量引擎:扫描、并发、落盘、歌词
src-tauri/               Tauri 外壳(命令层 + 窗口)
renderer/                界面(原生 HTML/CSS/JS,无框架)
test/                    界面测试 + 样本构造器
tools/                   造测试样本、生成图标
```

## 实现上的几点取舍

**1. 音频起点不靠猜，靠"投票"**
ncm 里「元数据段之后、封面图之前」的字段排布，不同实现写法不一致（长度字段在 +5 还是 +9、总跨度是 `9+len` 还是 `13+len`）。三种候选分别解出前 4 个字节，看哪个是 `fLaC` / `ID3` / MP3 帧同步。

**2. 不自造标签库**
ID3v2.3 和 Vorbis Comment 都是手写字节实现，没有引入 `lofty` 这类大依赖——格式本身不复杂，自己写反而能精确控制 UTF-16 编码和帧顺序。

**3. 密钥不写死在代码里**
QQ 音乐的 ekey 从文件尾部读、酷我的密钥从密文静音段反推，都不依赖从客户端提取的数据。

**4. 明确不支持需要联网取密钥的格式**
QQ 音乐 ≥ 19.57 的 musicex 尾部不保存密钥，必须登录客户端向服务器请求。本工具**不做这类请求**，遇到会直接报错——这不是技术做不到，是刻意不做。

## 已知限制

- 不支持 QQ 音乐的 musicex 格式（需联网取密钥）
- 不支持酷狗 `.kgma`、咪咕等格式
- 不做转码（MP3 ↔ FLAC 互转需要编码器，不在本项目范围）
- 不做封面图嵌入
- 不支持 Apple Music / Spotify 等真正 DRM 的内容

## 支持这个项目

如果这个工具帮你省了时间，可以请我喝杯咖啡：**[爱发电](https://afdian.com/a/aiyanjiangs)**

**完全自愿，不影响任何功能。** 本项目的所有功能永久免费，源码以 MIT 协议开放。

收到的赞助主要用于：测试用的设备与音乐文件、域名与签名证书、以及继续维护的时间。

> 请不要为了让某个格式优先支持而付费 —— 功能需求请走 [Issues](https://github.com/aiyanjiangs/tuneport/issues)，
> 我会按通用价值排期，与是否赞助无关。

## 相关文档

- [常见问题（FAQ）](docs/FAQ.md) —— 转换失败、报毒、数据位置、格式支持范围
- [更新日志](CHANGELOG.md)

## 贡献

欢迎提 Issue 和 PR。提交前请确保：

1. `cd core-rs && cargo test` 通过
2. `npm test` 通过
3. 新增格式要附带**合成样本**和端到端测试（见 `core-rs/tests/`）
4. 不要提交从商业客户端提取的密钥或数据

## 许可证

[MIT](LICENSE)

## 致谢

格式细节参考了社区公开的技术资料与独立实现（各自遵循其原始许可证），本项目为独立实现：

- `tc_tea`（MIT / Apache-2.0）：腾讯 TEA 变体的 Rust 实现
- 各格式的测试向量与尾部结构来自社区文档的交叉验证
