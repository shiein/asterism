use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::config::HubConfig;
use crate::db;
use crate::tls;

#[derive(Parser, Debug)]
#[command(name = "asterism-hub", about = "Asterism self-hosted hub", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 生成 config.toml 与自签名证书（仅用于本地/内网首次启动）
    Init(InitArgs),
    Serve(ServeArgs),
    Migrate(ConfigArgs),
    Backup(BackupArgs),
    Doctor(ConfigArgs),
}

#[derive(Args, Debug)]
pub struct InitArgs {
    #[arg(long, default_value = "./data")]
    pub data_dir: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub bind: String,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct BackupArgs {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub dest: PathBuf,
}

pub fn init(args: InitArgs) -> Result<()> {
    fs::create_dir_all(args.data_dir.join("blobs"))?;
    let config_path = args.data_dir.join("config.toml");
    let cert_path = args.data_dir.join("tls.cert");
    let key_path = args.data_dir.join("tls.key");

    if !cert_path.exists() || !key_path.exists() {
        tls::write_self_signed(&cert_path, &key_path)?;
        tracing::info!("wrote self-signed TLS material");
    }

    let secret = asterism_sync::pairing::generate_bootstrap_secret();
    let hash = hex::encode(asterism_sync::pairing::hash_bootstrap(&secret));
    let config = HubConfig {
        bind: args.bind,
        data_dir: args.data_dir.clone(),
        tls: crate::config::TlsConfig { cert: cert_path, key: key_path },
        bootstrap_secret_hash: Some(hash),
    };
    if !config_path.exists() {
        fs::write(&config_path, config.to_toml()?)?;
        tracing::info!(path = %config_path.display(), "wrote config");
        let secret_path = args.data_dir.join("bootstrap.secret");
        fs::write(&secret_path, format!("{secret}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600));
        }
        println!("Bootstrap secret (save once, required for the first device): {secret}");
    }
    db::migrate(&config.data_dir.join("hub.db"))?;
    println!("initialized hub data dir {}", args.data_dir.display());
    Ok(())
}

pub async fn serve(args: ServeArgs) -> Result<()> {
    let config = load_config(args.config)?;
    let runtime = crate::host::hub_boot_plan().mount().map_err(|err| anyhow::anyhow!(err))?;
    tracing::info!(order = ?runtime.boot_order(), "hub boot plan");
    let result = crate::server::run(config).await;
    drop(runtime);
    result
}

pub fn migrate(args: ConfigArgs) -> Result<()> {
    let config = load_config(args.config)?;
    db::migrate(&config.db_path())?;
    println!("migrated {}", config.db_path().display());
    Ok(())
}

pub fn backup(args: BackupArgs) -> Result<()> {
    let config = load_config(args.config)?;
    if args.dest.exists() {
        bail!("backup destination already exists: {}", args.dest.display());
    }
    let parent = args.dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = args.dest.file_name().and_then(|value| value.to_str()).unwrap_or("backup");
    let staging_path = parent.join(format!(".{name}.partial-{}", std::process::id()));
    if staging_path.exists() {
        bail!("stale backup staging directory exists: {}", staging_path.display());
    }
    fs::create_dir(&staging_path)?;
    let mut staging = BackupStaging { path: staging_path, committed: false };
    let target_root = &staging.path;

    db::backup(&config.db_path(), &target_root.join("hub.db"))?;
    let blob_ids = db::referenced_blob_ids(&target_root.join("hub.db"))?;
    let mut blobs = Vec::new();
    for id in blob_ids {
        let source = config.blob_root().join(&id);
        let committed = source.join("committed");
        let chunk_count: u32 = fs::read_to_string(&committed)
            .with_context(|| format!("referenced blob {id} is not committed"))?
            .trim()
            .parse()
            .with_context(|| format!("invalid commit marker for blob {id}"))?;
        let target = target_root.join("blobs").join(&id);
        fs::create_dir_all(&target)?;
        let mut chunks = Vec::with_capacity(chunk_count as usize);
        for index in 0..chunk_count {
            let name = format!("chunk_{index}");
            let from = source.join(&name);
            let to = target.join(&name);
            fs::copy(&from, &to).with_context(|| format!("copy blob {id} chunk {index}"))?;
            let source_hash = sha256_file(&from)?;
            let copied_hash = sha256_file(&to)?;
            if source_hash != copied_hash {
                bail!("blob {id} chunk {index} changed while backup was running");
            }
            chunks.push(BackupChunk {
                index,
                bytes: fs::metadata(&to)?.len(),
                sha256: copied_hash,
            });
        }
        fs::write(target.join("committed"), chunk_count.to_string())?;
        blobs.push(BackupBlob { id, chunks });
    }

    fs::copy(&config.tls.cert, target_root.join("tls.cert")).context("copy TLS certificate")?;
    fs::copy(&config.tls.key, target_root.join("tls.key")).context("copy TLS private key")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target_root.join("tls.key"), fs::Permissions::from_mode(0o600))?;
    }
    let restored = HubConfig {
        data_dir: args.dest.clone(),
        tls: crate::config::TlsConfig {
            cert: args.dest.join("tls.cert"),
            key: args.dest.join("tls.key"),
        },
        ..config
    };
    fs::write(target_root.join("config.toml"), restored.to_toml()?)?;
    let manifest = BackupManifest { version: 1, blobs };
    fs::write(target_root.join("backup-manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    fs::rename(&staging.path, &args.dest)?;
    staging.committed = true;
    println!("backup written to {}", args.dest.display());
    Ok(())
}

struct BackupStaging {
    path: PathBuf,
    committed: bool,
}

impl Drop for BackupStaging {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(serde::Serialize)]
struct BackupManifest {
    version: u32,
    blobs: Vec<BackupBlob>,
}

#[derive(serde::Serialize)]
struct BackupBlob {
    id: String,
    chunks: Vec<BackupChunk>,
}

#[derive(serde::Serialize)]
struct BackupChunk {
    index: u32,
    bytes: u64,
    sha256: String,
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}

pub fn doctor(args: ConfigArgs) -> Result<()> {
    let path = resolve_config_path(args.config)?;
    let config = HubConfig::load(&path)?;
    let mut failed = false;
    if !config.tls.cert.exists() || !config.tls.key.exists() {
        eprintln!("tls material missing");
        failed = true;
    } else if let Err(err) = tls::load_server_config(&config.tls.cert, &config.tls.key) {
        eprintln!("tls invalid: {err}");
        failed = true;
    }
    if let Err(err) = db::migrate(&config.db_path()) {
        eprintln!("database: {err}");
        failed = true;
    }
    if failed {
        bail!("doctor found problems");
    }
    println!("ok: {}", path.display());
    Ok(())
}

fn load_config(explicit: Option<PathBuf>) -> Result<HubConfig> {
    let path = resolve_config_path(explicit)?;
    HubConfig::load(&path)
}

fn resolve_config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let default = PathBuf::from("./data/config.toml");
    if default.exists() {
        return Ok(default);
    }
    bail!("missing --config and ./data/config.toml; run `asterism-hub init`")
}

impl HubConfig {
    fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("serialize config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_migrate_doctor_backup() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        init(InitArgs { data_dir: data.clone(), bind: "127.0.0.1:18787".into() }).unwrap();
        assert!(data.join("config.toml").exists());
        assert!(data.join("tls.cert").exists());
        assert!(data.join("hub.db").exists());

        let config = Some(data.join("config.toml"));
        migrate(ConfigArgs { config: config.clone() }).unwrap();
        doctor(ConfigArgs { config: config.clone() }).unwrap();

        let dest = dir.path().join("backup");
        backup(BackupArgs { config, dest: dest.clone() }).unwrap();
        assert!(dest.join("hub.db").exists());
        assert!(dest.join("tls.key").exists());
        assert!(dest.join("backup-manifest.json").exists());
        doctor(ConfigArgs { config: Some(dest.join("config.toml")) }).unwrap();
    }
}
