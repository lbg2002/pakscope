//! GitHub Release update checker, RFC 6234 SHA-256 verifier, and Polkit DEB installer.
//! Performs network requests ONLY when explicitly invoked by the user.
use serde::Deserialize;
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_SLUG: &str = "lbg2002/pakscope";
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/lbg2002/pakscope/releases/latest";
pub const RELEASES_LIST_API: &str = "https://api.github.com/repos/lbg2002/pakscope/releases?per_page=5";
pub const RELEASES_PAGE_URL: &str = "https://github.com/lbg2002/pakscope/releases";

const ALLOWED_HTTPS_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "codeload.github.com",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAssetInfo {
    pub name: String,
    pub download_url: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version: String,
    pub title: String,
    pub body: String,
    pub html_url: String,
    pub published_at: String,
    pub prerelease: bool,
    pub deb_asset: Option<ReleaseAssetInfo>,
    pub sha256_asset: Option<ReleaseAssetInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateCheckResult {
    UpdateAvailable(ReleaseInfo),
    UpToDate {
        current_version: String,
        latest: ReleaseInfo,
    },
    NoReleasesYet {
        current_version: String,
        releases_url: String,
    },
}

#[derive(Clone, Debug)]
pub struct DownloadedUpdate {
    pub deb_path: PathBuf,
    pub filename: String,
    pub version: String,
    pub sha256_hex: String,
    pub checksum_verified: bool,
    pub verification_note: String,
    pub install_command_preview: String,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// Strictly validate that a URL uses HTTPS and points to an official GitHub domain.
pub fn validate_github_https_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "仅允许安全的 HTTPS 更新地址".to_string())?;
    let host_port = rest.split('/').next().unwrap_or("");
    if host_port.is_empty() || host_port.contains('@') {
        return Err("无效的更新服务器域名".into());
    }
    let host = host_port.split(':').next().unwrap_or("").to_ascii_lowercase();
    if ALLOWED_HTTPS_HOSTS.contains(&host.as_str()) {
        Ok(())
    } else {
        Err(format!("已拦截非受信任的下载域名：{host}"))
    }
}

pub fn normalize_version(raw: &str) -> &str {
    raw.trim()
        .strip_prefix('v')
        .or_else(|| raw.trim().strip_prefix('V'))
        .unwrap_or_else(|| raw.trim())
}

/// Compare two semantic version strings (e.g. `v0.3.1` vs `0.3.0` or `0.3.0-rc.1` vs `0.3.0`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parse = |s: &str| -> (Vec<u64>, Option<String>) {
        let clean = normalize_version(s);
        let core_and_build = clean.split('+').next().unwrap_or(clean);
        let (nums_str, pre) = match core_and_build.split_once('-') {
            Some((n, p)) => (n, Some(p.to_ascii_lowercase())),
            None => (core_and_build, None),
        };
        let mut nums: Vec<u64> = nums_str
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect();
        while nums.len() < 3 {
            nums.push(0);
        }
        (nums, pre)
    };
    let (na, pa) = parse(a);
    let (nb, pb) = parse(b);
    let max_len = na.len().max(nb.len());
    for i in 0..max_len {
        let va = na.get(i).copied().unwrap_or(0);
        let vb = nb.get(i).copied().unwrap_or(0);
        match va.cmp(&vb) {
            Ordering::Equal => continue,
            ord => return ord,
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        // Stable release is newer than pre-release of the same core version
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(sa), Some(sb)) => sa.cmp(&sb),
    }
}

pub fn current_deb_arch() -> String {
    if let Ok(out) = Command::new("dpkg").arg("--print-architecture").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    match std::env::consts::ARCH {
        "x86_64" => "amd64".into(),
        "aarch64" => "arm64".into(),
        other => other.into(),
    }
}

fn select_assets(assets: &[GhAsset], arch: &str) -> (Option<ReleaseAssetInfo>, Option<ReleaseAssetInfo>) {
    let mut deb_choice: Option<&GhAsset> = None;
    let mut sha_choice: Option<&GhAsset> = None;

    for asset in assets {
        if validate_github_https_url(&asset.browser_download_url).is_err() {
            continue;
        }
        let lower = asset.name.to_ascii_lowercase();
        if lower == "sha256sums" || lower == "sha256sums.txt" || lower.ends_with(".sha256") {
            if sha_choice.is_none() || lower == "sha256sums" {
                sha_choice = Some(asset);
            }
            continue;
        }
        if lower.ends_with(".deb") {
            let matches_arch = lower.contains(&format!("_{arch}.deb"))
                || lower.contains(&format!("-{arch}.deb"))
                || lower.contains("_all.deb");
            if matches_arch {
                deb_choice = Some(asset);
            } else if deb_choice.is_none()
                && !lower.contains("_arm64.deb")
                && !lower.contains("_amd64.deb")
            {
                deb_choice = Some(asset);
            }
        }
    }

    let map_asset = |a: &GhAsset| ReleaseAssetInfo {
        name: a.name.clone(),
        download_url: a.browser_download_url.clone(),
        size_bytes: a.size,
    };
    (deb_choice.map(map_asset), sha_choice.map(map_asset))
}

fn convert_release(gh: GhRelease, arch: &str) -> ReleaseInfo {
    let version = normalize_version(&gh.tag_name).to_string();
    let title = gh
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| gh.tag_name.clone());
    let (deb_asset, sha256_asset) = select_assets(&gh.assets, arch);
    ReleaseInfo {
        tag_name: gh.tag_name,
        version,
        title,
        body: gh.body.unwrap_or_default().trim().to_string(),
        html_url: gh.html_url,
        published_at: gh
            .published_at
            .map(|s| s.split('T').next().unwrap_or(&s).to_string())
            .unwrap_or_else(|| "—".into()),
        prerelease: gh.prerelease,
        deb_asset,
        sha256_asset,
    }
}

fn https_get_text(url: &str) -> Result<(u16, String), String> {
    validate_github_https_url(url)?;
    let ua = format!("PakScope/{CURRENT_VERSION}");

    // 1. Try curl (supports clean HTTP status extraction)
    if let Ok(out) = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            &ua,
            "-w",
            "\n__HTTP_STATUS__:%{http_code}",
            url,
        ])
        .output()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some((body, code_str)) = stdout.rsplit_once("\n__HTTP_STATUS__:") {
            if let Ok(code) = code_str.trim().parse::<u16>() {
                if code > 0 {
                    return Ok((code, body.to_string()));
                }
            }
        }
    }

    // 2. Fallback to python3 urllib (pre-installed on Ubuntu Desktop)
    let py_script = r#"
import sys, urllib.request, urllib.error, ssl
url, ua = sys.argv[1], sys.argv[2]
req = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": ua})
ctx = ssl.create_default_context()
try:
    with urllib.request.urlopen(req, context=ctx, timeout=20) as r:
        body = r.read().decode("utf-8", errors="replace")
        sys.stdout.write(f"{r.status}\n{body}")
except urllib.error.HTTPError as e:
    body = e.read().decode("utf-8", errors="replace")
    sys.stdout.write(f"{e.code}\n{body}")
except Exception as e:
    sys.stderr.write(str(e))
    sys.exit(2)
"#;
    if let Ok(out) = Command::new("python3")
        .args(["-c", py_script, url, &ua])
        .output()
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some((status_line, body)) = stdout.split_once('\n') {
                if let Ok(code) = status_line.trim().parse::<u16>() {
                    return Ok((code, body.to_string()));
                }
            }
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !err.is_empty() {
                return Err(format!("网络请求失败：{err}"));
            }
        }
    }

    Err("无法连接 GitHub（请检查网络连接，或确认系统已安装 curl / python3）".into())
}

fn https_download_to_file(url: &str, dest: &Path) -> Result<(), String> {
    validate_github_https_url(url)?;
    let ua = format!("PakScope/{CURRENT_VERSION}");

    if let Ok(out) = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "180",
            "-A",
            &ua,
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .output()
    {
        if out.status.success() && dest.is_file() {
            return Ok(());
        }
    }

    let py_script = r#"
import sys, urllib.request, ssl
url, ua, dest = sys.argv[1], sys.argv[2], sys.argv[3]
req = urllib.request.Request(url, headers={"User-Agent": ua})
ctx = ssl.create_default_context()
with urllib.request.urlopen(req, context=ctx, timeout=180) as r, open(dest, "wb") as f:
    while True:
        chunk = r.read(65536)
        if not chunk:
            break
        f.write(chunk)
"#;
    let out = Command::new("python3")
        .args(["-c", py_script, url, &ua])
        .arg(dest)
        .output()
        .map_err(|e| format!("无法启动下载器：{e}"))?;
    if out.status.success() && dest.is_file() {
        Ok(())
    } else {
        Err(format!(
            "下载失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let arch = current_deb_arch();
    let (status, body) = https_get_text(LATEST_RELEASE_API)?;
    if status == 200 {
        let gh: GhRelease =
            serde_json::from_str(&body).map_err(|e| format!("解析 Release 数据失败：{e}"))?;
        let info = convert_release(gh, &arch);
        return Ok(classify_release(CURRENT_VERSION, info));
    }

    if status == 404 {
        let (list_status, list_body) = https_get_text(RELEASES_LIST_API)?;
        if list_status == 200 {
            let list: Vec<GhRelease> = serde_json::from_str(&list_body)
                .map_err(|e| format!("解析 Release 列表失败：{e}"))?;
            if let Some(first) = list.into_iter().find(|r| !r.draft) {
                let info = convert_release(first, &arch);
                return Ok(classify_release(CURRENT_VERSION, info));
            }
            return Ok(UpdateCheckResult::NoReleasesYet {
                current_version: CURRENT_VERSION.to_string(),
                releases_url: RELEASES_PAGE_URL.to_string(),
            });
        }
    }

    if status == 403 {
        return Err("GitHub API 访问频率受限（HTTP 403），请稍后再试或直接打开 Release 页面查看。".into());
    }

    Err(format!("GitHub Release 查询返回异常状态（HTTP {status}）"))
}

pub fn classify_release(current_version: &str, latest: ReleaseInfo) -> UpdateCheckResult {
    if compare_versions(&latest.version, current_version) == Ordering::Greater {
        UpdateCheckResult::UpdateAvailable(latest)
    } else {
        UpdateCheckResult::UpToDate {
            current_version: current_version.to_string(),
            latest,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure-Rust RFC 6234 SHA-256 implementation (streaming, zero external crates)
// ---------------------------------------------------------------------------
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

struct Sha256State {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Sha256State {
    fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    fn compress_block(h: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_bytes += data.len() as u64;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                Self::compress_block(&mut self.h, &self.buf);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            Self::compress_block(&mut self.h, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let bit_len = self.total_bytes * 8;
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            for b in &mut self.buf[self.buf_len..64] {
                *b = 0;
            }
            Self::compress_block(&mut self.h, &self.buf);
            self.buf_len = 0;
        }
        for b in &mut self.buf[self.buf_len..56] {
            *b = 0;
        }
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        Self::compress_block(&mut self.h, &self.buf);

        let mut out = String::with_capacity(64);
        for word in self.h {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }
}

pub fn sha256_bytes(data: &[u8]) -> String {
    let mut st = Sha256State::new();
    st.update(data);
    st.finalize_hex()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("无法读取下载包以计算校验值：{e}"))?;
    let mut st = Sha256State::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("读取文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        st.update(&buf[..n]);
    }
    Ok(st.finalize_hex())
}

/// Parse standard `SHA256SUMS` lines (`<64-hex>  <filename>` or `<64-hex> *<filename>`).
pub fn expected_sha256_for_asset(sums_text: &str, asset_name: &str) -> Option<String> {
    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let hash = parts.next()?;
        let raw_name = parts.next().unwrap_or("");
        let clean_name = raw_name
            .trim_start_matches('*')
            .rsplit('/')
            .next()
            .unwrap_or(raw_name);
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            if clean_name == asset_name || raw_name.is_empty() {
                return Some(hash.to_ascii_lowercase());
            }
        }
    }
    None
}

fn verify_deb_identity(deb_path: &Path) -> Result<(), String> {
    let out = Command::new("dpkg-deb")
        .args(["-f"])
        .arg(deb_path)
        .args(["Package", "Architecture"])
        .output()
        .map_err(|e| format!("无法调用 dpkg-deb 校验安装包结构：{e}"))?;
    if !out.status.success() {
        return Err("下载的文件不是有效的 Debian (.deb) 安装包".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pkg_ok = false;
    let mut arch_val = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Package:") {
            pkg_ok = rest.trim() == "pakscope";
        } else if let Some(rest) = line.strip_prefix("Architecture:") {
            arch_val = rest.trim().to_string();
        }
    }
    if !pkg_ok {
        return Err("安装包内部包名不是 pakscope，已拒绝安装".into());
    }
    let sys_arch = current_deb_arch();
    if !arch_val.is_empty() && arch_val != "all" && arch_val != sys_arch {
        return Err(format!(
            "安装包架构（{arch_val}）与当前系统（{sys_arch}）不兼容"
        ));
    }
    Ok(())
}

fn create_update_staging_dir() -> Result<PathBuf, String> {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("pakscope-update-{pid}-{nanos}"));
    fs::create_dir_all(&dir).map_err(|e| format!("创建临时更新目录失败：{e}"))?;
    // 0755 on staging dir so apt's unprivileged `_apt` user can read the verified .deb
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
    Ok(dir)
}

pub fn download_and_verify_update<F: FnMut(&str)>(
    release: &ReleaseInfo,
    mut on_progress: F,
) -> Result<DownloadedUpdate, String> {
    let deb_asset = release
        .deb_asset
        .as_ref()
        .ok_or("该 GitHub Release 未提供适用于当前系统架构的 .deb 安装包，请打开 Release 页面查看。")?;

    let safe_filename = deb_asset
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
        && deb_asset.name.ends_with(".deb")
        && !deb_asset.name.starts_with('.');
    if !safe_filename {
        return Err("远端安装包文件名不符合安全规范".into());
    }

    let staging = create_update_staging_dir()?;
    let deb_path = staging.join(&deb_asset.name);

    on_progress(&format!("正在下载 {} …", deb_asset.name));
    https_download_to_file(&deb_asset.download_url, &deb_path)?;
    let _ = fs::set_permissions(&deb_path, fs::Permissions::from_mode(0o644));

    on_progress("正在计算 SHA-256 哈希并校验 DEB 软件包结构…");
    let actual_sha = sha256_file(&deb_path)?;

    let (checksum_verified, verification_note) = if let Some(ref sha_asset) = release.sha256_asset {
        let (_, sums_body) = https_get_text(&sha_asset.download_url)?;
        let expected = expected_sha256_for_asset(&sums_body, &deb_asset.name)
            .ok_or_else(|| format!("SHA256SUMS 中未找到 {} 的摘要记录", deb_asset.name))?;
        if actual_sha != expected {
            let _ = fs::remove_file(&deb_path);
            return Err(format!(
                "SHA-256 校验不匹配！已拒绝并删除安装包。\n期望：{expected}\n实际：{actual_sha}"
            ));
        }
        (true, format!("SHA-256 摘要校验通过（{}…）", &actual_sha[..16]))
    } else {
        (
            false,
            format!(
                "已通过 DEB 结构与包名校验（SHA-256: {}…，该 Release 未附带 SHA256SUMS）",
                &actual_sha[..16]
            ),
        )
    };

    verify_deb_identity(&deb_path)?;

    let preview = format!("sudo apt-get install -y -- '{}'", deb_path.display());
    Ok(DownloadedUpdate {
        deb_path,
        filename: deb_asset.name.clone(),
        version: release.version.clone(),
        sha256_hex: actual_sha,
        checksum_verified,
        verification_note,
        install_command_preview: preview,
    })
}

pub fn install_downloaded_update(downloaded: &DownloadedUpdate) -> Result<String, String> {
    let meta = fs::symlink_metadata(&downloaded.deb_path)
        .map_err(|e| format!("更新包不存在或已被移除：{e}"))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("更新包文件状态异常（非普通文件），已拒绝安装".into());
    }
    let recheck_sha = sha256_file(&downloaded.deb_path)?;
    if recheck_sha != downloaded.sha256_hex {
        return Err("安装前二次校验发现文件哈希变化，已终止安装！".into());
    }
    verify_deb_identity(&downloaded.deb_path)?;

    let out = Command::new("/usr/bin/pkexec")
        .args([
            "/usr/bin/apt-get",
            "install",
            "-y",
            "--allow-downgrades",
            "--",
        ])
        .arg(&downloaded.deb_path)
        .output()
        .map_err(|e| format!("无法启动 Polkit 授权安装器（pkexec）：{e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(format!(
            "PakScope v{} 安装成功！重启应用即可加载新版本。\n{}",
            downloaded.version,
            stdout.chars().take(4000).collect::<String>()
        ))
    } else {
        Err(format!(
            "安装未完成（{}）：\n{}\n{}",
            out.status,
            stderr.trim(),
            stdout.trim()
        )
        .chars()
        .take(5000)
        .collect())
    }
}

pub fn save_deb_to_downloads(downloaded: &DownloadedUpdate) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("无法定位用户主目录")?;
    let target_dir = ["Downloads", "下载"]
        .iter()
        .map(|d| home.join(d))
        .find(|p| p.is_dir())
        .unwrap_or(home);
    let dest = target_dir.join(&downloaded.filename);
    fs::copy(&downloaded.deb_path, &dest).map_err(|e| format!("保存安装包失败：{e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nist_sha256_vectors() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn semver_comparisons_handle_prefixes_and_prereleases() {
        assert_eq!(compare_versions("v0.3.1", "0.3.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.3.0", "v0.3.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.3.0-rc.1", "0.3.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
    }

    #[test]
    fn github_https_url_allowlist() {
        assert!(validate_github_https_url(LATEST_RELEASE_API).is_ok());
        assert!(validate_github_https_url(
            "https://github.com/lbg2002/pakscope/releases/download/v0.3.0/pakscope_0.3.0_amd64.deb"
        )
        .is_ok());
        assert!(validate_github_https_url("http://github.com/lbg2002/pakscope").is_err());
        assert!(validate_github_https_url("https://evil.example.com/pakscope.deb").is_err());
        assert!(validate_github_https_url("https://github.com@evil.com/x.deb").is_err());
    }

    #[test]
    fn parse_sha256sums_formats() {
        let sums = "\
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  dist/pakscope_0.3.0_amd64.deb\n\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *other.deb\n";
        assert_eq!(
            expected_sha256_for_asset(sums, "pakscope_0.3.0_amd64.deb").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(expected_sha256_for_asset(sums, "missing.deb"), None);
    }
}
