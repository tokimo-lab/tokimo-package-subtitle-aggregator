pub mod addic7ed;
pub mod animekalesi;
pub mod animesubinfo;
pub mod animetosho;
pub mod assrt;
pub mod betaseries;
pub mod bsplayer;
pub mod gestdown;
pub mod greeksubs;
pub mod greeksubtitles;
pub mod hosszupuska;
pub mod jimaku;
pub mod ktuvit;
pub mod legendasdivx;
pub mod legendasnet;
pub mod napiprojekt;
pub mod napisy24;
pub mod nekur;
pub mod opensubtitles;
pub mod podnapisi;
pub mod regielive;
pub mod shooter;
pub mod soustitreseu;
pub mod subdl;
pub mod subf2m;
pub mod subs4free;
pub mod subs4series;
pub mod subscenter;
pub mod subsource;
pub mod subsro;
pub mod subssabbz;
pub mod subsunacs;
pub mod subsynchro;
pub mod subtis;
pub mod subtitrarinoi;
pub mod subtitriid;
pub mod subtitulamostv;
pub mod subx;
pub mod supersubtitles;
pub mod thesubdb;
pub mod titlovi;
pub mod titrari;
pub mod titulky;
pub mod turkcealtyazi;
pub mod tvsubtitles;
pub mod wizdom;
pub mod xsubs;
pub mod xunlei;
pub mod yavkanet;
pub mod yify;
pub mod zimuku;

use async_trait::async_trait;

use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

/// Common trait for all subtitle providers
#[async_trait]
pub trait SubtitleProvider: Send + Sync {
    /// Provider name (e.g. "assrt", "opensubtitles", "shooter")
    fn name(&self) -> &str;

    /// Search for subtitles
    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String>;

    /// Download a specific subtitle
    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String>;
}
