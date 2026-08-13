use std::fs;
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

    let config = HubConfig {
        bind: args.bind,
        data_dir: args.data_dir.clone(),
        tls: crate::config::TlsConfig { cert: cert_path, key: key_path },
    };
    if !config_path.exists() {
        fs::write(&config_path, config.to_toml()?)?;
        tracing::info!(path = %config_path.display(), "wrote config");
    }
    db::migrate(&config.data_dir.join("hub.db"))?;
    println!("initialized hub data dir {}", args.data_dir.display());
    Ok(())
}

pub async fn serve(args: ServeArgs) -> Result<()> {
    let config = load_config(args.config)?;
    crate::server::run(config).await
}

pub fn migrate(args: ConfigArgs) -> Result<()> {
    let config = load_config(args.config)?;
    db::migrate(&config.db_path())?;
    println!("migrated {}", config.db_path().display());
    Ok(())
}

pub fn backup(args: BackupArgs) -> Result<()> {
    let config_hint = args.config.clone();
    let config = load_config(args.config)?;
    fs::create_dir_all(&args.dest)?;
    db::backup(&config.db_path(), &args.dest.join("hub.db"))?;
    copy_dir(&config.data_dir.join("blobs"), &args.dest.join("blobs"))?;
    // 备份 config 但不复制私钥内容到 stdout。文件仍复制，dest 需受保护。
    if config.tls.cert.exists() {
        fs::copy(&config.tls.cert, args.dest.join("tls.cert"))?;
    }
    fs::copy(config_path_hint(config_hint.as_deref()), args.dest.join("config.toml")).ok();
    println!("backup written to {}", args.dest.display());
    Ok(())
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

fn config_path_hint(explicit: Option<&Path>) -> PathBuf {
    explicit.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("./data/config.toml"))
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
    }
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

impl HubConfig {
    fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self).context("serialize config")?)
    }
}
