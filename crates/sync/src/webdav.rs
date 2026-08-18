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
            // URL 路径分段编码，避免空格或特殊符号导致 URI 解析失败
            let encoded_segments: Vec<String> =
                clean_path.split('/').map(percent_encode_segment).collect();
            format!("{}/{}", self.base_url, encoded_segments.join("/"))
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
            || resp.status() == StatusCode::NOT_FOUND
        {
            Ok(())
        } else if resp.status() == StatusCode::UNAUTHORIZED
            || resp.status() == StatusCode::FORBIDDEN
        {
            Err(SyncError::Failed("WebDAV 认证失败，请检查用户名和密码".into()))
        } else {
            Err(SyncError::Failed(format!("WebDAV 状态码错误: {}", resp.status())))
        }
    }

    /// 创建目录（MKCOL）
    pub async fn mkcol(&self, path: &str) -> Result<()> {
        let mut url = self.full_url(path);
        if !url.ends_with('/') {
            url.push('/');
        }
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
        let mut url = self.full_url(path);
        if !url.ends_with('/') {
            url.push('/');
        }
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
        let self_folder = path.trim_matches('/').rsplit('/').next().unwrap_or("");
        let mut results = Vec::new();

        for href in extract_hrefs(&xml) {
            let decoded_href = decode_xml_entities(&href);
            let unquoted = percent_decode(&decoded_href);
            let item_name =
                unquoted.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
            // 排除自身目录项及空文件名
            if !item_name.is_empty() && item_name != self_folder && !results.contains(&item_name) {
                results.push(item_name);
            }
        }

        Ok(results)
    }
}

fn percent_encode_segment(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::new();
    let chars: Vec<u8> = s.bytes().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == b'%'
            && i + 2 < chars.len()
            && let Ok(val) =
                u8::from_str_radix(std::str::from_utf8(&chars[i + 1..i + 3]).unwrap_or(""), 16)
        {
            bytes.push(val);
            i += 3;
            continue;
        }
        bytes.push(chars[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn extract_hrefs(xml: &str) -> Vec<String> {
    let mut hrefs = Vec::new();
    let lower_xml = xml.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start_tag) = lower_xml[pos..].find('<') {
        let tag_start = pos + start_tag;
        if let Some(tag_end) = lower_xml[tag_start..].find('>') {
            let tag_content = &lower_xml[tag_start + 1..tag_start + tag_end];
            if tag_content == "href" || tag_content.ends_with(":href") {
                let val_start = tag_start + tag_end + 1;
                if let Some(close_tag) = lower_xml[val_start..].find("</") {
                    let href_val = xml[val_start..val_start + close_tag].trim();
                    if !href_val.is_empty() {
                        hrefs.push(href_val.to_string());
                    }
                    pos = val_start + close_tag;
                    continue;
                }
            }
            pos = tag_start + tag_end + 1;
        } else {
            break;
        }
    }
    hrefs
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
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
            client.full_url("blobs/test item.bin"),
            "https://dav.example.com/dav/Asterism/blobs/test%20item.bin"
        );
        assert!(client.auth_header.is_some());
    }

    #[test]
    fn href_extraction_and_decoding() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <D:multistatus xmlns:D="DAV:">
            <D:response>
                <D:href>/dav/Asterism/</D:href>
            </D:response>
            <D:response>
                <D:href>/dav/Asterism/hello%20world%20&amp;%20notes.png</D:href>
            </D:response>
        </D:multistatus>"#;
        let hrefs = extract_hrefs(xml);
        assert_eq!(hrefs.len(), 2);
        let name = decode_xml_entities(&hrefs[1]);
        let unquoted = percent_decode(&name);
        assert_eq!(unquoted, "/dav/Asterism/hello world & notes.png");
    }
}
