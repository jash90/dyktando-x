//! Model catalog and downloading. Each model is a set of files in `models/<dir>/`;
//! a model is installed when all its files exist (downloads go to `.part`).
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

use crate::i18n::{t, t_with};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    ParakeetV3,
    CanaryV2,
    WhisperTurbo,
    WhisperLargeV3,
}

impl EngineId {
    pub const ALL: [EngineId; 4] = [Self::ParakeetV3, Self::CanaryV2, Self::WhisperTurbo, Self::WhisperLargeV3];

    pub fn asset(self) -> &'static Asset {
        asset(AssetId::Engine(self))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "engine")]
pub enum AssetId {
    Engine(EngineId),
    /// Silero VAD v4 — splitting meeting recordings into utterances.
    SileroVad,
    /// WeSpeaker ResNet34 — speaker recognition in meeting recordings.
    SpeakerModel,
}

pub struct RemoteFile {
    pub url: &'static str,
    pub name: &'static str,
    pub size: u64,
}

pub struct Asset {
    pub id: AssetId,
    pub dir: &'static str,
    /// Proper name of the model (also stored with transcripts and dictations).
    pub title: &'static str,
    /// Translation key prefix: `model.<key>.description` (and `model.<key>.title` where the title
    /// isn't a proper name).
    pub key: &'static str,
    pub files: &'static [RemoteFile],
}

macro_rules! parakeet {
    ($f:literal, $size:expr) => {
        RemoteFile {
            url: concat!("https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/", $f),
            name: $f,
            size: $size,
        }
    };
}

macro_rules! canary {
    ($f:literal, $size:expr) => {
        RemoteFile {
            url: concat!("https://huggingface.co/istupakov/canary-1b-v2-onnx/resolve/main/", $f),
            name: $f,
            size: $size,
        }
    };
}

pub static ASSETS: &[Asset] = &[
    Asset {
        id: AssetId::Engine(EngineId::ParakeetV3),
        dir: "parakeet-tdt-0.6b-v3-int8",
        title: "Parakeet TDT 0.6B v3",
        key: "parakeet_v3",
        files: &[
            parakeet!("encoder-model.int8.onnx", 652_183_999),
            parakeet!("decoder_joint-model.int8.onnx", 18_202_004),
            parakeet!("nemo128.onnx", 139_764),
            parakeet!("vocab.txt", 93_939),
        ],
    },
    Asset {
        id: AssetId::Engine(EngineId::CanaryV2),
        dir: "canary-1b-v2-int8",
        title: "Canary 1B v2",
        key: "canary_v2",
        files: &[
            canary!("encoder-model.int8.onnx", 859_078_138),
            canary!("decoder-model.int8.onnx", 170_040_374),
            canary!("vocab.txt", 208_022),
            // The same 128-mel preprocessor as Parakeet (the Canary repo doesn't include it).
            parakeet!("nemo128.onnx", 139_764),
        ],
    },
    Asset {
        id: AssetId::Engine(EngineId::WhisperTurbo),
        dir: "whisper-large-v3-turbo",
        title: "Whisper large-v3-turbo",
        key: "whisper_turbo",
        files: &[RemoteFile {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            name: "ggml-large-v3-turbo.bin",
            size: 1_624_555_275,
        }],
    },
    Asset {
        id: AssetId::Engine(EngineId::WhisperLargeV3),
        dir: "whisper-large-v3",
        title: "Whisper large-v3",
        key: "whisper_large_v3",
        files: &[RemoteFile {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",
            name: "ggml-large-v3-q5_0.bin",
            size: 1_081_140_203,
        }],
    },
    Asset {
        id: AssetId::SileroVad,
        dir: "silero-vad",
        title: "Silero VAD",
        key: "silero_vad",
        files: &[RemoteFile {
            url: "https://github.com/snakers4/silero-vad/raw/v4.0/files/silero_vad.onnx",
            name: "silero_vad.onnx",
            size: 1_807_522,
        }],
    },
    Asset {
        id: AssetId::SpeakerModel,
        dir: "wespeaker-resnet34",
        title: "Speaker recognition",
        key: "speaker_model",
        files: &[RemoteFile {
            url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx",
            name: "wespeaker_en_voxceleb_resnet34_LM.onnx",
            size: 26_530_550,
        }],
    },
];

pub fn asset(id: AssetId) -> &'static Asset {
    ASSETS.iter().find(|a| a.id == id).expect("every AssetId has an entry in ASSETS")
}

impl Asset {
    /// Title for the UI in the current language.
    pub fn label(&self) -> String {
        let key = format!("model.{}.title", self.key);
        let text = t(&key);
        if text == key { self.title.to_string() } else { text }
    }

    /// Description for the UI in the current language.
    pub fn description(&self) -> String {
        t(&format!("model.{}.description", self.key))
    }

    pub fn dir_path(&self) -> PathBuf {
        paths::models().join(self.dir)
    }

    pub fn file_path(&self, index: usize) -> PathBuf {
        self.dir_path().join(self.files[index].name)
    }

    pub fn is_installed(&self) -> bool {
        (0..self.files.len()).all(|i| self.file_path(i).is_file())
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn remove(&self) -> Result<()> {
        let dir = self.dir_path();
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| t_with("model.removing", &[("path", &dir.display())]))?;
        }
        Ok(())
    }

    /// Downloads missing files. `progress(downloaded, total)` — in bytes for the whole model.
    /// `cancel` aborts between data chunks (the `.part` file stays for resuming… from scratch).
    pub async fn download(&self, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<()> {
        let dir = self.dir_path();
        tokio::fs::create_dir_all(&dir).await?;
        let total = self.total_size();
        let client = reqwest::Client::builder()
            .user_agent(concat!("DyktandoX/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let mut done: u64 = 0;
        for (i, file) in self.files.iter().enumerate() {
            let target = self.file_path(i);
            if target.is_file() {
                done += file.size;
                progress(done, total);
                continue;
            }
            let part = target.with_extension("part");
            let response = client
                .get(file.url)
                .send()
                .await
                .with_context(|| t_with("model.downloading", &[("file", &file.name)]))?;
            if !response.status().is_success() {
                return Err(anyhow!("{}: HTTP {}", file.name, response.status()));
            }
            let mut out = tokio::fs::File::create(&part).await?;
            let mut stream = response.bytes_stream();
            let mut written: u64 = 0;
            while let Some(chunk) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    drop(out);
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(anyhow!(t("model.download_cancelled")));
                }
                let chunk = chunk.with_context(|| t_with("model.downloading", &[("file", &file.name)]))?;
                out.write_all(&chunk).await?;
                written += chunk.len() as u64;
                progress(done + written, total);
            }
            out.flush().await?;
            drop(out);
            if written != file.size {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(anyhow!(t_with("model.download_incomplete", &[("file", &file.name), ("done", &written), ("total", &file.size)])));
            }
            tokio::fs::rename(&part, &target).await?;
            done += file.size;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_asset() {
        for e in EngineId::ALL {
            assert!(!e.asset().files.is_empty());
        }
        assert_eq!(asset(AssetId::SileroVad).files.len(), 1);
        for a in ASSETS {
            let key = format!("model.{}.description", a.key);
            assert_ne!(t(&key), key, "missing translation {key}");
        }
    }

    #[test]
    fn asset_id_json() {
        let j = serde_json::to_string(&AssetId::Engine(EngineId::WhisperTurbo)).unwrap();
        assert_eq!(j, r#"{"kind":"engine","engine":"whisper_turbo"}"#);
        assert_eq!(serde_json::from_str::<AssetId>(r#"{"kind":"silero_vad"}"#).unwrap(), AssetId::SileroVad);
    }
}
