use std::path::Path;
use std::{fs::File, io::Read};

use sevenz_rust::decompress_file as decompress_7z_file;
use tokio::fs;
use unrar::Archive as UnrarArchive;
use uuid::Uuid;
use zip::ZipArchive;

use crate::models::{normalize_format, score_subtitle_name, DownloadedSubtitle};

#[derive(Debug, Clone)]
struct ExtractedSubtitleFile {
    name: String,
    format: String,
    content: Vec<u8>,
}

pub fn normalize_format_from_name(name: &str) -> Option<String> {
    normalize_format(name)
}

fn archive_extension(archive_name: &str) -> Option<String> {
    archive_name
        .rsplit('.')
        .next()
        .filter(|value| !value.is_empty() && *value != archive_name)
        .map(|value| value.to_ascii_lowercase())
}

fn temporary_archive_name(archive_name: &str) -> String {
    let extension = archive_name
        .rsplit('.')
        .next()
        .filter(|value| !value.is_empty() && *value != archive_name)
        .unwrap_or("bin");
    format!("archive.{extension}")
}

fn read_extracted_subtitles_from_dir(root: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut extracted_files = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries =
            std::fs::read_dir(&directory).map_err(|error| format!("读取解压目录失败: {error}"))?;

        for entry in entries {
            let entry = entry.map_err(|error| format!("遍历解压目录失败: {error}"))?;
            let path = entry.path();

            if path.is_dir() {
                directories.push(path);
                continue;
            }

            if !path.is_file() {
                continue;
            }

            let file_name = path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "subtitle".into());
            let Some(format) = normalize_format(&file_name) else {
                continue;
            };
            let content =
                std::fs::read(&path).map_err(|error| format!("读取解压后的字幕失败: {error}"))?;
            extracted_files.push(ExtractedSubtitleFile {
                name: file_name,
                format,
                content,
            });
        }
    }

    Ok(extracted_files)
}

fn extract_rar_subtitles(
    archive_path: &Path,
    output_dir: &Path,
) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let archive = UnrarArchive::new(archive_path).as_first_part();
    let mut archive = archive
        .open_for_processing()
        .map_err(|error| format!("解压 RAR 字幕失败: {error}"))?;

    while let Some(header) = archive
        .read_header()
        .map_err(|error| format!("读取 RAR 条目失败: {error}"))?
    {
        archive = if header.entry().is_file() {
            let destination = output_dir.join(&header.entry().filename);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建 RAR 解压目录失败: {error}"))?;
            }
            header
                .extract_to(&destination)
                .map_err(|error| format!("提取 RAR 字幕失败: {error}"))?
        } else {
            header
                .skip()
                .map_err(|error| format!("跳过 RAR 条目失败: {error}"))?
        };
    }

    read_extracted_subtitles_from_dir(output_dir)
}

fn extract_zip_subtitles(archive_path: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let file = File::open(archive_path).map_err(|error| format!("打开 ZIP 压缩包失败: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("读取 ZIP 压缩包失败: {error}"))?;
    let mut extracted_files = Vec::new();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 ZIP 条目失败: {error}"))?;
        if !entry.is_file() {
            continue;
        }

        let file_name = entry
            .enclosed_name()
            .and_then(|path| {
                path.file_name()
                    .map(|value| value.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| entry.name().to_string());
        let Some(format) = normalize_format(&file_name) else {
            continue;
        };

        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|error| format!("读取 ZIP 字幕内容失败: {error}"))?;
        extracted_files.push(ExtractedSubtitleFile {
            name: file_name,
            format,
            content,
        });
    }

    Ok(extracted_files)
}

fn extract_7z_subtitles(
    archive_path: &Path,
    output_dir: &Path,
) -> Result<Vec<ExtractedSubtitleFile>, String> {
    decompress_7z_file(archive_path, output_dir)
        .map_err(|error| format!("解压 7z 字幕失败: {error}"))?;

    read_extracted_subtitles_from_dir(output_dir)
}

pub async fn extract_archive(
    archive_bytes: &[u8],
    archive_name: &str,
    preferred_language: &str,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let temp_root = staging_root
        .join("subtitle-extract")
        .join(Uuid::new_v4().to_string());
    let result = async {
        let output_dir = temp_root.join("out");
        fs::create_dir_all(&output_dir)
            .await
            .map_err(|error| format!("创建临时目录失败: {error}"))?;

        let archive_path = temp_root.join(temporary_archive_name(archive_name));
        fs::write(&archive_path, archive_bytes)
            .await
            .map_err(|error| format!("写入压缩包失败: {error}"))?;
        let archive_path_for_task = archive_path.clone();
        let output_dir_for_task = output_dir.clone();
        let archive_name_for_task = archive_name.to_string();
        let extracted_files = tokio::task::spawn_blocking(move || {
            if UnrarArchive::new(&archive_path_for_task).is_archive() {
                return extract_rar_subtitles(&archive_path_for_task, &output_dir_for_task);
            }

            match archive_extension(&archive_name_for_task).as_deref() {
                Some("zip") => extract_zip_subtitles(&archive_path_for_task),
                Some("7z") => extract_7z_subtitles(&archive_path_for_task, &output_dir_for_task),
                Some(extension) => Err(format!("暂不支持解压 {extension} 格式的字幕包")),
                None => Err("无法识别字幕压缩包格式".into()),
            }
        })
        .await
        .map_err(|error| format!("解压字幕任务失败: {error}"))??;

        let selected = extracted_files
            .into_iter()
            .max_by_key(|file| score_subtitle_name(&file.name, &file.format, preferred_language))
            .ok_or_else(|| "压缩包内未找到可用字幕文件".to_string())?;

        Ok(DownloadedSubtitle {
            name: selected.name,
            format: selected.format,
            content: selected.content,
        })
    }
    .await;

    let _ = fs::remove_dir_all(&temp_root).await;
    result
}
