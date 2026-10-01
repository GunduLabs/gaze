# Gaze 中文 PAM 提示与 KRDP 登录修复

基于 [gundulabs/gaze v0.3.5](https://github.com/gundulabs/gaze/tree/v0.3.5) 的源码分支，包含 PAM 认证提示汉化，以及 KDE 远程桌面登录时跳过本机人脸扫描的修复。

本项目独立于远程桌面网页认证项目，可单独查看、构建或提取补丁。源代码已经应用下面两份补丁，不需要再次应用。

## 改动

### 中文 PAM 认证提示

将 PAM 层面向用户的提示改为中文，包括看向摄像头、输入密码、人脸已验证、未识别到人脸、光线不足、超时以及钥匙环解锁提示；同步调整相关测试。

汉化范围集中在 `pam-gaze/src/auth.rs` 和 `pam-gaze/src/core.rs` 的认证交互提示。Gaze 的 GUI、CLI 和上游文档不属于本次完整汉化范围。

### KRDP 远程登录修复

部分 KRDP 版本通过通用的 `login` PAM 服务认证，却不设置 `PAM_RHOST`，导致 Gaze 误把网络登录当成本地登录，启动摄像头并等待识别。

补丁仅在 PAM 服务为 `login` 且实际运行的可执行文件路径为 `/usr/bin/krdpserver` 时，让 Gaze 返回 `PAM_IGNORE`。之后继续由原 PAM 密码与账户栈判定登录结果；补丁本身不会直接授予登录权限。本地控制台、锁屏、sudo 等仍走各自原有认证流程。

## 文件

| 文件 | 说明 |
| --- | --- |
| `pam-gaze/src/auth.rs`、`pam-gaze/src/core.rs` | 已应用汉化和修复的源文件 |
| `patches/0001-zh-cn-pam.patch` | 相对上游 v0.3.5 的中文 PAM 提示补丁 |
| `patches/0002-krdp-skip-face-auth.patch` | 在第一份补丁之后应用的 KRDP 修复 |
| `patches/series` | 补丁应用顺序 |
| `UPSTREAM.json` | 上游地址、精确提交和许可证 |
| `docs/upstream-readme.md` | 原始上游项目介绍 |
| `LICENSE` | 上游 GPL 许可证全文 |

仓库保留上游 Rust 工作区、依赖锁文件、打包和构建脚本；不包含本机人脸数据、PAM 配置、账户密码、旧二进制、Git 历史、下载的编译依赖或 `target/` 目录。上游发布工作流未复制，避免新仓库误用原项目的发布配置。

## 构建与测试

使用支持 Rust 2024 edition 的工具链，推荐通过 rustup 安装当前稳定版。PAM 模块的构建需要 C 编译工具、pkg-config 和 PAM 开发库。Fedora 可先安装：

```bash
sudo dnf install gcc pkgconf-pkg-config pam-devel
cargo build --release --locked -p pam-gaze
cargo test --locked -p pam-gaze
```

其他发行版请安装对应的 PAM 开发包，例如 Debian/Ubuntu 的 `libpam0g-dev`。构建产物为 `target/release/libpam_gaze.so`。如果要构建整个 Gaze，请参考[上游构建说明](https://gaze.gundulabs.com/guide/development)与仓库 `Justfile`，其他组件可能需要额外依赖。

本次补丁开发时，PAM 模块的 74 项测试通过；导出时再次校验了两份补丁按顺序应用到上游基线后，与本项目两个修改后的 Rust 文件逐字节一致。

## 安装到已经运行 Gaze 的主机

以下针对 Fedora x86_64，假设已有兼容的 Gaze v0.3.5 和正常的 PAM 配置。PAM 属于登录组件，替换前请保留一个可用的管理员终端，并备份现有模块。示例不修改系统 PAM 策略，也不安装整套 Gaze。

```bash
gaze_backup_dir="/var/backups/gaze-zh-rdp-$(date +%Y%m%d-%H%M%S)"
sudo install -d -m 700 "$gaze_backup_dir"
sudo cp -a /usr/lib64/security/pam_gaze.so "$gaze_backup_dir/pam_gaze.so"
sudo install -o root -g root -m 755 target/release/libpam_gaze.so /usr/lib64/security/pam_gaze.so
sudo restorecon -F /usr/lib64/security/pam_gaze.so
```

其他发行版应使用实际 PAM 模块目录。安装后在保留的管理员会话之外验证本地认证和新的远程桌面登录。已经运行的进程可能仍映射旧模块，需要结束旧会话或重启对应 KRDP 用户服务后再验证。

恢复时，将备份的 `pam_gaze.so` 复制回同一路径并恢复 SELinux 标签。发行版或上游更新 Gaze 软件包可能覆盖本地模块，升级后需重新核对补丁兼容性。

## 将补丁应用到上游源码

从上游检出 `UPSTREAM.json` 记录的 v0.3.5 提交，在该上游工作区中按顺序执行：

```bash
git apply /path/to/gaze-zh-rdp/patches/0001-zh-cn-pam.patch
git apply /path/to/gaze-zh-rdp/patches/0002-krdp-skip-face-auth.patch
```

随后重新构建。其他上游版本可能需要调整补丁。KRDP 如果安装在不同路径，应先核实实际可执行文件路径，再修改 `is_krdp_network_login` 并同步测试；不要仅根据进程名或所有 `login` PAM 请求跳过人脸识别。

## 许可证

沿用上游 **GPL-3.0-or-later**，保留源文件中的 Gundu Labs 版权与 SPDX 声明。汉化和远程登录修复也按相同许可证提供。项目没有改变或代表上游的官方发布。
