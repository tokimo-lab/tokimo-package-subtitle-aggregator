use std::sync::Arc;

use subtitle_aggregator::{
    aggregator::SubtitleAggregator,
    models::SubtitleSearchRequest,
    providers::{
        addic7ed::Addic7edProvider, animekalesi::AnimekalesiProvider,
        animesubinfo::AnimesubinfoProvider, animetosho::AnimeToshoProvider, assrt::AssrtProvider,
        betaseries::BetaSeriesProvider, bsplayer::BsPlayerProvider, gestdown::GestdownProvider,
        greeksubs::GreekSubsProvider, greeksubtitles::GreekSubtitlesProvider,
        hosszupuska::HosszupuskaProvider, jimaku::JimakuProvider, ktuvit::KtuvitProvider,
        legendasdivx::LegendasDivxProvider, legendasnet::LegendasNetProvider,
        napiprojekt::NapiprojektProvider, napisy24::Napisy24Provider, nekur::NekurProvider,
        opensubtitles::OpenSubtitlesProvider, podnapisi::PodnapisiProvider,
        regielive::RegieLiveProvider, shooter::ShooterProvider, soustitreseu::SoustitreseuProvider,
        subdl::SubdlProvider, subf2m::Subf2mProvider, subs4free::Subs4FreeProvider,
        subs4series::Subs4SeriesProvider, subscenter::SubsCenterProvider,
        subsource::SubSourceProvider, subsro::SubsRoProvider, subssabbz::SubssabbzProvider,
        subsunacs::SubsunacsProvider, subsynchro::SubsynchroProvider, subtis::SubtisProvider,
        subtitrarinoi::SubtitrariNoiProvider, subtitriid::SubtitriIdProvider,
        subtitulamostv::SubtitulamosTvProvider, subx::SubxProvider,
        supersubtitles::SuperSubtitlesProvider, thesubdb::TheSubDbProvider,
        titlovi::TitloviProvider, titrari::TitrariProvider, titulky::TitulkyProvider,
        turkcealtyazi::TurkcealtyaziProvider, tvsubtitles::TvSubtitlesProvider,
        wizdom::WizdomProvider, xsubs::XSubsProvider, xunlei::XunleiSubtitleProvider,
        yavkanet::YavkanetProvider, yify::YifyProvider, zimuku::ZimukuProvider,
    },
};
use tracing_subscriber::EnvFilter;

fn build_aggregator() -> SubtitleAggregator {
    let mut aggregator = SubtitleAggregator::new();
    let staging = "/tmp/subtitle-aggregator";

    // ── Always-on: Chinese / Asian ──────────────────────────────────────────
    aggregator.add_provider(Arc::new(AssrtProvider::new(staging)));
    aggregator.add_provider(Arc::new(ZimukuProvider::new(staging)));
    aggregator.add_provider(Arc::new(AnimekalesiProvider::new()));
    aggregator.add_provider(Arc::new(AnimesubinfoProvider::new()));
    aggregator.add_provider(Arc::new(AnimeToshoProvider::new()));

    // ── Always-on: English / multi-language ─────────────────────────────────
    aggregator.add_provider(Arc::new(PodnapisiProvider::new()));
    aggregator.add_provider(Arc::new(SubdlProvider::new(
        std::env::var("SUBDL_API_KEY").ok(),
    )));
    aggregator.add_provider(Arc::new(Subf2mProvider::new(staging)));
    aggregator.add_provider(Arc::new(YifyProvider::new(staging)));
    aggregator.add_provider(Arc::new(TvSubtitlesProvider::new()));
    aggregator.add_provider(Arc::new(BsPlayerProvider::new()));
    aggregator.add_provider(Arc::new(NapiprojektProvider::new()));
    aggregator.add_provider(Arc::new(Napisy24Provider::new()));
    aggregator.add_provider(Arc::new(NekurProvider::new()));
    aggregator.add_provider(Arc::new(SubtitriIdProvider::new()));
    aggregator.add_provider(Arc::new(SuperSubtitlesProvider::new()));
    aggregator.add_provider(Arc::new(HosszupuskaProvider::new()));
    aggregator.add_provider(Arc::new(GestdownProvider::new()));
    aggregator.add_provider(Arc::new(SubtisProvider::new()));
    aggregator.add_provider(Arc::new(SubtitulamosTvProvider::new()));

    // ── Always-on: French ───────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(SoustitreseuProvider::new(staging)));
    aggregator.add_provider(Arc::new(SubsynchroProvider::new(staging)));

    // ── Always-on: Portuguese ───────────────────────────────────────────────
    aggregator.add_provider(Arc::new(LegendasDivxProvider::new(staging)));
    aggregator.add_provider(Arc::new(LegendasNetProvider::new(staging)));

    // ── Always-on: Greek ────────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(GreekSubsProvider::new(staging)));
    aggregator.add_provider(Arc::new(GreekSubtitlesProvider::new(staging)));
    aggregator.add_provider(Arc::new(Subs4FreeProvider::new(staging)));
    aggregator.add_provider(Arc::new(Subs4SeriesProvider::new(staging)));
    aggregator.add_provider(Arc::new(XSubsProvider::new(staging)));
    aggregator.add_provider(Arc::new(SubsCenterProvider::new(staging)));

    // ── Always-on: Hebrew ───────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(WizdomProvider::new(staging)));

    // ── Always-on: Romanian ─────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(TitrariProvider::new(staging)));
    aggregator.add_provider(Arc::new(RegieLiveProvider::new(staging)));
    aggregator.add_provider(Arc::new(SubtitrariNoiProvider::new(staging)));
    aggregator.add_provider(Arc::new(SubsRoProvider::new(staging)));

    // ── Always-on: Turkish ──────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(TurkcealtyaziProvider::new()));

    // ── Always-on: Bulgarian ────────────────────────────────────────────────
    aggregator.add_provider(Arc::new(SubssabbzProvider::new()));
    aggregator.add_provider(Arc::new(SubsunacsProvider::new()));
    aggregator.add_provider(Arc::new(YavkanetProvider::new()));

    // ── Optional credential providers ───────────────────────────────────────
    aggregator.add_provider(Arc::new(Addic7edProvider::new(
        std::env::var("ADDIC7ED_USER").ok(),
        std::env::var("ADDIC7ED_PASS").ok(),
    )));
    aggregator.add_provider(Arc::new(SubSourceProvider::new(
        std::env::var("SUBSOURCE_API_KEY").ok(),
    )));
    aggregator.add_provider(Arc::new(SubxProvider::new()));
    aggregator.add_provider(Arc::new(BetaSeriesProvider::new(staging)));
    aggregator.add_provider(Arc::new(KtuvitProvider::new(staging)));

    if let Ok(api_key) = std::env::var("JIMAKU_API_KEY") {
        if !api_key.is_empty() {
            aggregator.add_provider(Arc::new(JimakuProvider::new(api_key)));
        }
    }

    if let Ok(user) = std::env::var("TITLOVI_USER") {
        if let Ok(pass) = std::env::var("TITLOVI_PASS") {
            aggregator.add_provider(Arc::new(TitloviProvider::new(user, pass, staging)));
        }
    }

    if let Ok(user) = std::env::var("TITULKY_USER") {
        if let Ok(pass) = std::env::var("TITULKY_PASS") {
            aggregator.add_provider(Arc::new(TitulkyProvider::new(user, pass, staging)));
        }
    }

    if let Ok(api_key) = std::env::var("OPENSUBTITLES_API_KEY") {
        if !api_key.is_empty() {
            aggregator.add_provider(Arc::new(OpenSubtitlesProvider::new(api_key)));
        }
    }

    // ── Hash-based providers (skip silently if no file_hash) ────────────────
    aggregator.add_provider(Arc::new(ShooterProvider::new()));
    aggregator.add_provider(Arc::new(XunleiSubtitleProvider::new()));
    aggregator.add_provider(Arc::new(TheSubDbProvider::new()));

    aggregator
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let aggregator = build_aggregator();

    eprintln!("\n已注册 providers: {:?}", aggregator.provider_names());
    eprintln!("──────────────────────────────────────");

    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Inception".into());
    eprintln!("\n搜索: {query}\n");

    let request = SubtitleSearchRequest {
        query: Some(query),
        imdb_id: None,
        tmdb_id: None,
        languages: Some(vec!["zh-CN".into(), "zh".into(), "en".into()]),
        file_hash: None,
        file_size: None,
    };

    match aggregator.search(&request).await {
        Ok(results) => {
            eprintln!("共搜索到 {} 条字幕结果:\n", results.len());
            for (i, r) in results.iter().enumerate() {
                println!(
                    "[{:>2}] [{:<14}] [{:<6}] [{:<5}] {}{}",
                    i + 1,
                    r.provider,
                    r.language,
                    r.format,
                    r.name,
                    r.download_count
                        .map(|c| format!("  (↓{c})"))
                        .unwrap_or_default()
                );
            }
            eprintln!(
                "\nJSON: {} bytes",
                serde_json::to_string(&results)
                    .map(|s| s.len())
                    .unwrap_or(0)
            );
        }
        Err(e) => eprintln!("搜索失败: {e}"),
    }
}
