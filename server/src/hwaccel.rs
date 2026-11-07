use anyhow::Result;
use serde::Serialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Api {
    Nvenc,
    Vaapi,
    Qsv,
    Amf,
    VideoToolbox,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Nvidia,
    Intel,
    Amd,
    Apple,
    Cpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuKind {
    Integrated,
    Discrete,
    Cpu,
}

/// Source formats we test hardware decoding for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeCodec {
    H264,
    Hevc,
    Hevc10,
    Vp9,
    Vp9_10,
    Av1,
    Av1_10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodeCodec {
    H264,
    Hevc,
    Hevc10,
    Av1,
}

impl DecodeCodec {
    pub const ALL: [DecodeCodec; 7] = [Self::H264, Self::Hevc, Self::Hevc10, Self::Vp9, Self::Vp9_10, Self::Av1, Self::Av1_10];

    /// Map an ffprobe codec name and bit depth to a decode capability.
    pub fn from_probe(codec: &str, bit_depth: Option<i64>) -> Option<Self> {
        let deep = bit_depth.unwrap_or(8) > 8;
        Some(match codec {
            "h264" => Self::H264,
            "hevc" if deep => Self::Hevc10,
            "hevc" => Self::Hevc,
            "vp9" if deep => Self::Vp9_10,
            "vp9" => Self::Vp9,
            "av1" if deep => Self::Av1_10,
            "av1" => Self::Av1,
            _ => return None,
        })
    }

    fn sample_args(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::H264 => ("libx264", "yuv420p", "h264.mkv"),
            Self::Hevc => ("libx265", "yuv420p", "hevc.mkv"),
            Self::Hevc10 => ("libx265", "yuv420p10le", "hevc10.mkv"),
            Self::Vp9 => ("libvpx-vp9", "yuv420p", "vp9.webm"),
            Self::Vp9_10 => ("libvpx-vp9", "yuv420p10le", "vp9_10.webm"),
            Self::Av1 => ("libsvtav1", "yuv420p", "av1.mkv"),
            Self::Av1_10 => ("libsvtav1", "yuv420p10le", "av1_10.mkv"),
        }
    }

    fn is_10bit(self) -> bool {
        matches!(self, Self::Hevc10 | Self::Vp9_10 | Self::Av1_10)
    }
}

#[derive(Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub vendor: Vendor,
    pub kind: GpuKind,
    pub api: Api,
    pub render_node: Option<PathBuf>,
    pub cuda_index: Option<u32>,
    pub decode: Vec<DecodeCodec>,
    pub encode: Vec<EncodeCodec>,
    pub hw_tonemap: bool,
    pub hw_scale: bool,
    pub max_sessions: usize,
    #[serde(serialize_with = "ser_active")]
    active: AtomicUsize,
}

fn ser_active<S: serde::Serializer>(a: &AtomicUsize, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(a.load(Ordering::Relaxed) as u64)
}

impl Device {
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn label(&self) -> String {
        let api = match self.api {
            Api::Nvenc => "NVENC",
            Api::Vaapi if self.vendor == Vendor::Intel => "Quick Sync (VA-API)",
            Api::Vaapi => "VA-API",
            Api::Qsv => "Quick Sync",
            Api::Amf => "AMF",
            Api::VideoToolbox => "VideoToolbox",
            Api::Software => "CPU",
        };
        format!("{} · {api}", self.name)
    }
}

/// Holds a slot on a device for the lifetime of a job.
pub struct Lease {
    pub device: Arc<Device>,
    pub hw_decode: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.device.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Real-time streaming: discrete GPUs first.
    Stream,
    /// Thumbnails and other background work: integrated GPUs first, keep the discrete GPU free.
    Background,
}

pub struct Hardware {
    pub ffmpeg: String,
    pub devices: Vec<Arc<Device>>,
    pub filters: HashSet<String>,
}

impl Hardware {
    pub fn can_encode_in_hardware(&self, codec: EncodeCodec) -> bool {
        self.devices.iter().any(|d| d.api != Api::Software && d.encode.contains(&codec))
    }

    pub fn has_filter(&self, f: &str) -> bool {
        self.filters.contains(f)
    }

    pub fn software(&self) -> Arc<Device> {
        self.devices.iter().find(|d| d.api == Api::Software).cloned().expect("software device")
    }

    /// Least-loaded device that can encode `target`, preferring one that can also decode `source`
    /// and, when HDR must become SDR, one that tone maps on the GPU.
    pub fn cpu_lease(&self) -> Lease {
        let device = self.software();
        device.active.fetch_add(1, Ordering::Relaxed);
        Lease { device, hw_decode: false }
    }

    pub fn acquire(&self, source: Option<DecodeCodec>, target: EncodeCodec, purpose: Purpose, tonemap: bool) -> Lease {
        let kind_rank = |d: &Device| match (purpose, d.kind) {
            (_, GpuKind::Cpu) => 9,
            (Purpose::Stream, GpuKind::Discrete) | (Purpose::Background, GpuKind::Integrated) => 0,
            _ => 1,
        };
        let pick = self
            .devices
            .iter()
            .filter(|d| d.api != Api::Software && d.encode.contains(&target) && d.active() < d.max_sessions)
            .map(|d| {
                let hw_decode = source.is_some_and(|s| d.decode.contains(&s));
                (d, hw_decode)
            })
            .min_by_key(|(d, hw_decode)| (!hw_decode, tonemap && !d.hw_tonemap, d.active(), kind_rank(d)));
        let (device, hw_decode) = match pick {
            Some((d, hw)) => (d.clone(), hw),
            None => (self.software(), false),
        };
        device.active.fetch_add(1, Ordering::Relaxed);
        Lease { device, hw_decode }
    }

    /// Device for decode-only work (thumbnails), no encoder needed.
    pub fn acquire_decoder(&self, source: Option<DecodeCodec>, purpose: Purpose) -> Lease {
        let kind_rank = |d: &Device| match (purpose, d.kind) {
            (Purpose::Background, GpuKind::Integrated) | (Purpose::Stream, GpuKind::Discrete) => 0,
            _ => 1,
        };
        let pick = source.and_then(|s| {
            self.devices
                .iter()
                .filter(|d| d.api != Api::Software && d.decode.contains(&s) && d.active() < d.max_sessions)
                .min_by_key(|d| (d.active(), kind_rank(d)))
                .cloned()
        });
        let (device, hw_decode) = match pick {
            Some(d) => (d, true),
            None => (self.software(), false),
        };
        device.active.fetch_add(1, Ordering::Relaxed);
        Lease { device, hw_decode }
    }
}

struct Candidate {
    name: String,
    vendor: Vendor,
    kind: GpuKind,
    api: Api,
    render_node: Option<PathBuf>,
    cuda_index: Option<u32>,
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// GPUs visible through /dev/dri (Linux) plus NVIDIA via CUDA.
fn linux_candidates() -> Vec<Candidate> {
    let mut out = vec![];
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD")))
        .collect();
    nodes.sort();
    let mut nvidia_seen = 0u32;
    let nvidia_names = nvidia_smi_names();

    for node in nodes {
        let name = node.file_name().unwrap().to_string_lossy().to_string();
        let sys = PathBuf::from("/sys/class/drm").join(&name).join("device");
        let vendor_id = read_trim(sys.join("vendor")).unwrap_or_default();
        let pci = std::fs::read_link(&sys).ok().and_then(|l| l.file_name().map(|f| f.to_string_lossy().to_string())).unwrap_or_default();
        let on_root_bus = pci.starts_with("0000:00:");
        match vendor_id.as_str() {
            "0x8086" => out.push(Candidate {
                name: if on_root_bus { "Intel iGPU".into() } else { "Intel Arc".into() },
                vendor: Vendor::Intel,
                kind: if on_root_bus { GpuKind::Integrated } else { GpuKind::Discrete },
                api: Api::Vaapi,
                render_node: Some(node.clone()),
                cuda_index: None,
            }),
            "0x1002" => {
                let vram = read_trim(sys.join("mem_info_vram_total")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                let integrated = vram > 0 && vram <= 2 << 30;
                let product = read_trim(sys.join("product_name")).filter(|p| !p.is_empty());
                out.push(Candidate {
                    name: product.unwrap_or_else(|| if integrated { "AMD iGPU".into() } else { "AMD Radeon".into() }),
                    vendor: Vendor::Amd,
                    kind: if integrated { GpuKind::Integrated } else { GpuKind::Discrete },
                    api: Api::Vaapi,
                    render_node: Some(node.clone()),
                    cuda_index: None,
                });
            }
            "0x10de" => {
                out.push(Candidate {
                    name: nvidia_names.get(nvidia_seen as usize).cloned().unwrap_or_else(|| "NVIDIA GPU".into()),
                    vendor: Vendor::Nvidia,
                    kind: GpuKind::Discrete,
                    api: Api::Nvenc,
                    render_node: Some(node.clone()),
                    cuda_index: Some(nvidia_seen),
                });
                nvidia_seen += 1;
            }
            _ => {}
        }
    }
    // Containers with the NVIDIA runtime may expose CUDA without a matching render node.
    for (i, name) in nvidia_names.iter().enumerate().skip(nvidia_seen as usize) {
        out.push(Candidate {
            name: name.clone(),
            vendor: Vendor::Nvidia,
            kind: GpuKind::Discrete,
            api: Api::Nvenc,
            render_node: None,
            cuda_index: Some(i as u32),
        });
    }
    out
}

fn nvidia_smi_names() -> Vec<String> {
    std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

fn windows_candidates() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = nvidia_smi_names()
        .into_iter()
        .enumerate()
        .map(|(i, name)| Candidate { name, vendor: Vendor::Nvidia, kind: GpuKind::Discrete, api: Api::Nvenc, render_node: None, cuda_index: Some(i as u32) })
        .collect();
    out.push(Candidate { name: "Intel GPU".into(), vendor: Vendor::Intel, kind: GpuKind::Integrated, api: Api::Qsv, render_node: None, cuda_index: None });
    out.push(Candidate { name: "AMD GPU".into(), vendor: Vendor::Amd, kind: GpuKind::Discrete, api: Api::Amf, render_node: None, cuda_index: None });
    out
}

/// Input-side hardware arguments: device init and decode.
pub fn decode_args(d: &Device, hw_frames: bool) -> Vec<String> {
    let mut a: Vec<String> = vec![];
    let node = || d.render_node.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    match d.api {
        Api::Nvenc => {
            let i = d.cuda_index.unwrap_or(0);
            a.extend(["-init_hw_device".into(), format!("cuda=cu:{i}"), "-filter_hw_device".into(), "cu".into()]);
            a.extend(["-hwaccel".into(), "cuda".into(), "-hwaccel_device".into(), "cu".into()]);
            if hw_frames {
                a.extend(["-hwaccel_output_format".into(), "cuda".into()]);
            }
        }
        Api::Vaapi => {
            a.extend(["-init_hw_device".into(), format!("vaapi=va:{}", node()), "-filter_hw_device".into(), "va".into()]);
            a.extend(["-hwaccel".into(), "vaapi".into(), "-hwaccel_device".into(), "va".into()]);
            if hw_frames {
                a.extend(["-hwaccel_output_format".into(), "vaapi".into()]);
            }
        }
        Api::Qsv => {
            a.extend(["-init_hw_device".into(), "d3d11va=dx".into(), "-init_hw_device".into(), "qsv=qs@dx".into(), "-filter_hw_device".into(), "qs".into()]);
            a.extend(["-hwaccel".into(), "qsv".into(), "-hwaccel_device".into(), "qs".into()]);
            if hw_frames {
                a.extend(["-hwaccel_output_format".into(), "qsv".into()]);
            }
        }
        Api::Amf => {
            a.extend(["-hwaccel".into(), "d3d11va".into()]);
        }
        Api::VideoToolbox => {
            a.extend(["-hwaccel".into(), "videotoolbox".into()]);
            if hw_frames {
                a.extend(["-hwaccel_output_format".into(), "videotoolbox_vld".into()]);
            }
        }
        Api::Software => {}
    }
    a
}

/// Device init only, for jobs that upload software frames (no hardware decode).
pub fn init_args(d: &Device) -> Vec<String> {
    let node = d.render_node.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    match d.api {
        Api::Nvenc => vec!["-init_hw_device".into(), format!("cuda=cu:{}", d.cuda_index.unwrap_or(0)), "-filter_hw_device".into(), "cu".into()],
        Api::Vaapi => vec!["-init_hw_device".into(), format!("vaapi=va:{node}"), "-filter_hw_device".into(), "va".into()],
        Api::Qsv => vec!["-init_hw_device".into(), "d3d11va=dx".into(), "-init_hw_device".into(), "qsv=qs@dx".into(), "-filter_hw_device".into(), "qs".into()],
        _ => vec![],
    }
}

pub fn encoder_name(api: Api, codec: EncodeCodec) -> &'static str {
    use EncodeCodec::*;
    match (api, codec) {
        (Api::Nvenc, H264) => "h264_nvenc",
        (Api::Nvenc, Hevc | Hevc10) => "hevc_nvenc",
        (Api::Nvenc, Av1) => "av1_nvenc",
        (Api::Vaapi, H264) => "h264_vaapi",
        (Api::Vaapi, Hevc | Hevc10) => "hevc_vaapi",
        (Api::Vaapi, Av1) => "av1_vaapi",
        (Api::Qsv, H264) => "h264_qsv",
        (Api::Qsv, Hevc | Hevc10) => "hevc_qsv",
        (Api::Qsv, Av1) => "av1_qsv",
        (Api::Amf, H264) => "h264_amf",
        (Api::Amf, Hevc | Hevc10) => "hevc_amf",
        (Api::Amf, Av1) => "av1_amf",
        (Api::VideoToolbox, H264) => "h264_videotoolbox",
        (Api::VideoToolbox, Hevc | Hevc10) => "hevc_videotoolbox",
        (Api::VideoToolbox, Av1) => "av1_videotoolbox",
        (Api::Software, H264) => "libx264",
        (Api::Software, Hevc | Hevc10) => "libx265",
        (Api::Software, Av1) => "libsvtav1",
    }
}

/// Filter that turns software frames into this device's hardware frames (for encoders that need them).
pub fn upload_filter(api: Api, ten_bit: bool) -> String {
    let fmt = if ten_bit { "p010le" } else { "nv12" };
    match api {
        Api::Nvenc => format!("format={fmt},hwupload_cuda"),
        Api::Vaapi => format!("format={fmt},hwupload"),
        Api::Qsv => format!("format={fmt},hwupload=extra_hw_frames=64"),
        _ => format!("format={fmt}"),
    }
}

async fn run_ok(ffmpeg: &str, args: &[String]) -> bool {
    let fut = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error"])
        .args(args)
        .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();
    // Exit status only: some encoders (SVT-AV1) always print banners to stderr.
    match tokio::time::timeout(Duration::from_secs(30), fut).await {
        Ok(Ok(o)) => o.status.success(),
        _ => false,
    }
}

async fn ffmpeg_list(ffmpeg: &str, what: &str) -> String {
    tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", what])
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

async fn make_samples(ffmpeg: &str, dir: &Path, encoders: &str) -> Vec<(DecodeCodec, PathBuf)> {
    let _ = tokio::fs::create_dir_all(dir).await;
    let mut out = vec![];
    for codec in DecodeCodec::ALL {
        let (enc, pix, file) = codec.sample_args();
        if !encoders.contains(&format!(" {enc} ")) {
            continue;
        }
        let path = dir.join(file);
        if !path.exists() {
            let mut args: Vec<String> = ["-f", "lavfi", "-i", "testsrc2=s=640x360:r=24:d=1", "-pix_fmt", pix, "-c:v", enc].iter().map(|s| s.to_string()).collect();
            if enc == "libsvtav1" {
                args.extend(["-preset".into(), "12".into()]);
            }
            if enc == "libx265" {
                args.extend(["-x265-params".into(), "log-level=error".into()]);
            }
            args.extend(["-y".into(), path.display().to_string()]);
            if !run_ok(ffmpeg, &args).await {
                tracing::debug!("could not create {file} sample");
                continue;
            }
        }
        out.push((codec, path));
    }
    out
}

async fn verify(ffmpeg: &str, c: &Candidate, samples: &[(DecodeCodec, PathBuf)], encoders: &str, filters: &HashSet<String>) -> Option<Device> {
    let probe = Device {
        id: String::new(),
        name: c.name.clone(),
        vendor: c.vendor,
        kind: c.kind,
        api: c.api,
        render_node: c.render_node.clone(),
        cuda_index: c.cuda_index,
        decode: vec![],
        encode: vec![],
        hw_tonemap: false,
        hw_scale: false,
        max_sessions: 0,
        active: AtomicUsize::new(0),
    };

    let mut encode = vec![];
    for codec in [EncodeCodec::H264, EncodeCodec::Hevc, EncodeCodec::Hevc10, EncodeCodec::Av1] {
        let enc = encoder_name(c.api, codec);
        if !encoders.contains(&format!(" {enc} ")) {
            continue;
        }
        let ten = codec == EncodeCodec::Hevc10;
        let mut args = init_args(&probe);
        args.extend(["-f".into(), "lavfi".into(), "-i".into(), "testsrc2=s=1280x720:r=24:d=1".into()]);
        args.extend(["-vf".into(), upload_filter(c.api, ten)]);
        args.extend(["-c:v".into(), enc.into()]);
        if ten && c.api == Api::Nvenc {
            args.extend(["-profile:v".into(), "main10".into()]);
        }
        if c.api == Api::VideoToolbox {
            args.extend(["-allow_sw".into(), "0".into()]);
            if ten {
                args.extend(["-profile:v".into(), "main10".into()]);
            }
        }
        args.extend(["-frames:v".into(), "12".into(), "-f".into(), "null".into(), "-".into()]);
        if run_ok(ffmpeg, &args).await {
            encode.push(codec);
        }
    }

    let mut decode = vec![];
    for (codec, path) in samples {
        if matches!(c.api, Api::Amf) {
            break;
        }
        let fmt = if codec.is_10bit() { "p010le" } else { "nv12" };
        let mut args = decode_args(&probe, true);
        args.extend(["-i".into(), path.display().to_string()]);
        args.extend(["-vf".into(), format!("hwdownload,format={fmt}")]);
        args.extend(["-f".into(), "null".into(), "-".into()]);
        if run_ok(ffmpeg, &args).await {
            decode.push(*codec);
        }
    }

    if encode.is_empty() && decode.is_empty() {
        return None;
    }
    let (hw_scale, hw_tonemap) = match c.api {
        Api::Nvenc => (filters.contains("scale_cuda"), filters.contains("tonemap_cuda")),
        Api::Vaapi => (filters.contains("scale_vaapi"), c.vendor == Vendor::Intel && filters.contains("tonemap_vaapi")),
        Api::Qsv => (filters.contains("vpp_qsv"), filters.contains("vpp_qsv")),
        Api::VideoToolbox => (filters.contains("scale_vt"), filters.contains("scale_vt")),
        _ => (false, false),
    };
    let id = match (c.api, &c.render_node, c.cuda_index) {
        (Api::Nvenc, _, Some(i)) => format!("nvenc:{i}"),
        (api, Some(node), _) => format!("{}:{}", serde_json::to_value(api).unwrap().as_str().unwrap(), node.file_name().unwrap().to_string_lossy()),
        (api, _, _) => serde_json::to_value(api).unwrap().as_str().unwrap().to_string(),
    };
    Some(Device {
        id,
        decode,
        encode,
        hw_scale,
        hw_tonemap,
        max_sessions: match c.kind {
            GpuKind::Discrete => 8,
            _ => 4,
        },
        ..probe
    })
}

/// Detect and verify every hardware encoder/decoder by actually running ffmpeg on it.
pub async fn detect(ffmpeg: &str, work_dir: &Path, mode: &str) -> Result<Hardware> {
    let encoders = ffmpeg_list(ffmpeg, "-encoders").await;
    let filters: HashSet<String> = ffmpeg_list(ffmpeg, "-filters")
        .await
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect();

    let mut candidates = if mode == "none" {
        vec![]
    } else if cfg!(target_os = "macos") {
        vec![Candidate { name: "Apple GPU".into(), vendor: Vendor::Apple, kind: GpuKind::Integrated, api: Api::VideoToolbox, render_node: None, cuda_index: None }]
    } else if cfg!(windows) {
        windows_candidates()
    } else {
        linux_candidates()
    };
    if mode != "auto" && mode != "none" {
        let wanted: Vec<&str> = mode.split(',').map(str::trim).collect();
        candidates.retain(|c| {
            let api = serde_json::to_value(c.api).unwrap().as_str().unwrap().to_string();
            let vendor = serde_json::to_value(c.vendor).unwrap().as_str().unwrap().to_string();
            wanted.iter().any(|w| *w == api || *w == vendor)
        });
    }

    let samples = make_samples(ffmpeg, &work_dir.join("hwtest"), &encoders).await;
    let mut devices = vec![];
    for c in &candidates {
        match verify(ffmpeg, c, &samples, &encoders, &filters).await {
            Some(d) => {
                tracing::info!(device = %d.label(), kind = ?d.kind, decode = ?d.decode, encode = ?d.encode, tonemap = d.hw_tonemap, "hardware acceleration available");
                devices.push(Arc::new(d));
            }
            None => tracing::info!(name = %c.name, api = ?c.api, "hardware not usable"),
        }
    }
    let cpu_encode: Vec<EncodeCodec> = [("libx264", EncodeCodec::H264), ("libx265", EncodeCodec::Hevc), ("libx265", EncodeCodec::Hevc10), ("libsvtav1", EncodeCodec::Av1)]
        .into_iter()
        .filter(|(e, _)| encoders.contains(&format!(" {e} ")))
        .map(|(_, c)| c)
        .collect();
    devices.push(Arc::new(Device {
        id: "software".into(),
        name: "CPU".into(),
        vendor: Vendor::Cpu,
        kind: GpuKind::Cpu,
        api: Api::Software,
        render_node: None,
        cuda_index: None,
        decode: DecodeCodec::ALL.to_vec(),
        encode: cpu_encode,
        hw_tonemap: false,
        hw_scale: false,
        max_sessions: usize::MAX,
        active: AtomicUsize::new(0),
    }));
    Ok(Hardware { ffmpeg: ffmpeg.to_string(), devices, filters })
}
