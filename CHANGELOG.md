# 更新日志

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/)。

## [1.0.0] - 2026-11-04

首个公开发布版本。

### 新增

**格式支持**

- 网易云音乐 `.ncm`（AES-128-ECB + 改版 RC4，流式解密）
- QQ 音乐旧版 `.qmc` `.qmc0` `.qmc2` `.qmc3` `.qmcflac` `.qmcogg`（固定表按偏移异或）
- QQ 音乐新版 `.mflac` `.mflac0` `.mflach` `.mgg` `.mgg0` `.mgg1` `.mggl`（尾部 ekey → base64 → TC-TEA → 映射密码 / 改版 RC4）
- 酷我音乐 `.kwm`（`yeelion-kuwo` 头 + 32 字节循环密钥，从密文静音段恢复）

**转换与标签**

- 流式处理：ncm 按 1 MB 分块边解边写，内存不随文件变大
- 写标签：MP3 → ID3v2.3 / UTF-16；FLAC → Vorbis Comment（均为手写字节实现，不依赖第三方标签库）
- 读标签：FLAC Vorbis Comment、MP3 ID3v2.3/2.4、Ogg Vorbis 与 Opus 头
- 输出格式由解密后的文件头判定，不做转码
- 命名模板（`{artist}` `{title}` `{album}`），自动清洗 Windows 非法字符，重名自动加 ` (2)`

**界面**

- 批量拖入文件或文件夹（递归扫描）
- 失败行标红置顶，可单独重试
- 详情面板：元数据、输出路径、大小、耗时
- 内置试听播放器（读取字节转 Blob，不落地临时文件）
- 清空列表可撤销（6 秒）
- 设置面板：命名模板、同名策略、输出目录、并发数、完成后打开目录、默认复制歌词

**稳定性**

- 断点续跑：列表与每个文件状态存 `queue.json`，重开程序自动接上
- 启动日志写入 `%LOCALAPPDATA%\com.tuneport.app\tuneport-startup.log`
- WebView2 数据目录多级回退，受限环境给出明确提示
- 窗口创建失败/崩溃弹原生对话框，不再静默退出

**打包**

- NSIS 安装包（用户级安装，不需要管理员权限）
- 便携 zip（解压即用，不写注册表）

### 已知限制

- 不支持 QQ 音乐 ≥ 19.57 的 `musicex` 格式（密钥不存于文件，需联网获取；本项目刻意不做联网请求）
- 不支持酷狗 `.kgma`、咪咕等格式
- 不做转码（MP3 ↔ FLAC 互转）、不做封面图嵌入
- 仅 Windows（Tauri 本身跨平台，但本项目只提供 Windows 产物）
- 未做代码签名，首次运行可能触发 SmartScreen 提示

### 安全与合规

- 全程离线：不联网、不上传、不下载任何内容
- 不提供任何密钥获取途径：不从客户端内存或数据库提取密钥，不调用官方 API
- 源码以 MIT 协议开源
