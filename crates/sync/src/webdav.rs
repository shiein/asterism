use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{Result, SyncError};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebdavConfig {
    pub enabled: bool,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WebdavClient {
    base_url: String,
    auth_header: Option<String>,
    client: Client,
}

impl WebdavClient {
    pub fn new(config: &WebdavConfig) -> Result<Self> {
        let mut url = config.url.trim().to_string();
        if url.ends_with('/') {
            url.pop();
        }
        if url.is_empty() {
            return Err(SyncError::Failed("empty WebDAV url".into()));
        }

        let auth_header = match (&config.username, &config.password) {
            (Some(u), Some(p)) if !u.is_empty() => {
                let cred = format!("{}:{}", u, p);
                Some(format!("Basic {}", BASE64.encode(cred.as_bytes())))
            }
            _ => None,
        };

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| SyncError::Failed(e.to_string()))?;

        Ok(Self { base_url: url, auth_header, client })
    }

    fn full_url(&self, path: &str) -> String {
        let clean_path = path.trim_start_matches('/');
        if clean_path.is_empty() {
            self.base_url.clone()
        } else {
            format!("{}/{}", self.base_url, clean_path)
        }
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref auth) = self.auth_header {
            req.header(reqwest::header::AUTHORIZATION, auth)
        } else {
            req
        }
    }

    /// 测试连接与认证有效性（PROPFIND Depth 0）
    pub async fn test_connection(&self) -> Result<()> {
        let url = self.full_url("");
        let req = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "0");
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status().is_success()
            || resp.status() == StatusCode::MULTI_STATUS
            || resp.status() == StatusCode::METHOD_NOT_ALLOWED
        {
            Ok(())
        } else if resp.status() == StatusCode::UNAUTHORIZED {
            Err(SyncError::Failed("WebDAV 认证失败，请检查用户名和密码".into()))
        } else {
            Err(SyncError::Failed(format!("WebDAV 状态码错误: {}", resp.status())))
        }
    }

    /// 创建目录（MKCOL）
    pub async fn mkcol(&self, path: &str) -> Result<()> {
        let url = self.full_url(path);
        let req = self.client.request(Method::from_bytes(b"MKCOL").unwrap(), &url);
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status().is_success()
            || resp.status() == StatusCode::CREATED
            || resp.status() == StatusCode::METHOD_NOT_ALLOWED
        // 目录已存在
        {
            Ok(())
        } else {
            Err(SyncError::Failed(format!("MKCOL {} 失败: {}", path, resp.status())))
        }
    }

    /// 写入文件（PUT）
    pub async fn put(&self, path: &str, data: Vec<u8>) -> Result<()> {
        let url = self.full_url(path);
        let req = self.client.put(&url).body(data);
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status().is_success()
            || resp.status() == StatusCode::CREATED
            || resp.status() == StatusCode::NO_CONTENT
        {
            Ok(())
        } else {
            Err(SyncError::Failed(format!("PUT {} 失败: {}", path, resp.status())))
        }
    }

    /// 读取文件（GET）
    pub async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let url = self.full_url(path);
        let req = self.client.get(&url);
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(SyncError::Failed(format!("GET {} 失败: {}", path, resp.status())));
        }

        let bytes = resp.bytes().await.map_err(|e| SyncError::Failed(e.to_string()))?;
        Ok(Some(bytes.to_vec()))
    }

    /// 删除文件或目录（DELETE）
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = self.full_url(path);
        let req = self.client.delete(&url);
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status().is_success()
            || resp.status() == StatusCode::NOT_FOUND
            || resp.status() == StatusCode::NO_CONTENT
        {
            Ok(())
        } else {
            Err(SyncError::Failed(format!("DELETE {} 失败: {}", path, resp.status())))
        }
    }

    /// 列出目录下的文件名列表（PROPFIND Depth 1）
    pub async fn list(&self, path: &str) -> Result<Vec<String>> {
        let url = self.full_url(path);
        let req = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "1");
        let resp =
            self.apply_auth(req).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }

        if !resp.status().is_success() && resp.status() != StatusCode::MULTI_STATUS {
            return Err(SyncError::Failed(format!("PROPFIND {} 失败: {}", path, resp.status())));
        }

        let xml = resp.text().await.map_err(|e| SyncError::Failed(e.to_string()))?;
        let mut results = Vec::new();

        // 简易 href 提取器（避免引入庞大 XML 解析依赖）
        for chunk in xml.split("<D:href>").skip(1) {
            if let Some(end) = chunk.find("</D:href>") {
                let href = &chunk[..end];
                let item_name =
                    href.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
                if !item_name.is_empty() && !results.contains(&item_name) {
                    results.push(item_name);
                }
            }
        }
        for chunk in xml.split("<d:href>").skip(1) {
            if let Some(end) = chunk.find("</d:href>") {
                let href = &chunk[..end];
                let item_name =
                    href.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
                if !item_name.is_empty() && !results.contains(&item_name) {
                    results.push(item_name);
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_url_normalization() {
        let cfg = WebdavConfig {
            enabled: true,
            url: "https://dav.example.com/dav/Asterism/".into(),
            username: Some("user".into()),
            password: Some("pass".into()),
        };
        let client = WebdavClient::new(&cfg).unwrap();
        assert_eq!(
            client.full_url("blobs/123.bin"),
            "https://dav.example.com/dav/Asterism/blobs/123.bin"
        );
        assert!(client.auth_header.is_some());
    }
}
