use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use serde::Deserialize;
use sqlx::{postgres::PgPoolOptions,  Row};
use std::env;
use std::thread;
use std::time::Duration;
use tracing::{info, warn, error};
use metrics::{counter, gauge};
use metrics_exporter_prometheus::PrometheusBuilder;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use dotenvy::dotenv;

#[derive(Deserialize)]
struct Symbol {
    symbol: String,
    mic: Option<String>,
    currency: Option<String>,
}

#[derive(Deserialize)]
struct Profile {
    name: Option<String>,
    country: Option<String>,
    ipo: Option<String>,
    #[serde(rename = "marketCapitalization")]
    market_cap: Option<f64>,
    #[serde(rename = "finnhubIndustry")]
    industry: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SymbolMaster {
    symbol: String,
    exchange: Option<String>,
    name: Option<String>,
    sector: Option<String>,
    industry: Option<String>,
    currency: Option<String>,
    country: Option<String>,
    ipo_date: Option<NaiveDate>,
    market_cap: Option<i64>,
    is_active: bool,
    data_source: String,
    last_updated: DateTime<Utc>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenv().ok(); // Ignore if .env file is missing
    info!("Loaded .env file for local testing");

    // Initialize logging and metrics
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    PrometheusBuilder::new().install()?;
    info!("Starting symbol_master sync at {}", Utc::now());

    // Initialize rate limiter (60 calls/min)
    let limiter = RateLimiter::direct(Quota::per_minute(NonZeroU32::new(60).unwrap()));

    // Load environment variables
    let api_key = env::var("FINNHUB_API_KEY").expect("FINNHUB_API_KEY must be set");
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // Initialize HTTP client and DB pool
    let client = Client::new();

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let response = client
        .get("https://finnhub.io/api/v1/stock/symbol")
        .query(&[("exchange", "US"), ("token", &api_key)])
        .send()
        .await?; // <-- await here
    let symbols: Vec<Symbol> = response.json().await?;
    gauge!("symbol_sync_total_symbols", symbols.len() as f64);

    // Get existing active symbols
    let existing_symbols: Vec<String> = sqlx::query("SELECT symbol FROM symbol_master WHERE is_active = TRUE")
        .fetch_all(&pool)
        .await?
        .into_iter()
        .map(|row| row.get("symbol"))
        .collect();

    // Identify new and delisted symbols
    let symbol_set: std::collections::HashSet<String> = symbols.iter().map(|s| s.symbol.clone()).collect();
    let existing_set: std::collections::HashSet<String> = existing_symbols.into_iter().collect();
    let new_symbols: Vec<&Symbol> = symbols.iter().filter(|s| !existing_set.contains(&s.symbol)).collect();
    let delisted_symbols: Vec<String> = existing_set.difference(&symbol_set).cloned().collect();
    gauge!("symbol_sync_new_symbols", new_symbols.len() as f64);
    gauge!("symbol_sync_delisted_symbols", delisted_symbols.len() as f64);

    // Process new symbols
    let mut records = Vec::new();
    for symbol in new_symbols {
        limiter.until_ready().await;
        let mut retries = 3;
        info!("Processing symbol: {}", symbol.symbol);
        let profile: Profile = loop {
            match client
                .get("https://finnhub.io/api/v1/stock/profile2")
                .query(&[("symbol", &symbol.symbol), ("token", &api_key)])
                .send()
                .await
            {
                Ok(response) => {
                    if response.status() == 429 {
                        warn!("Rate limit hit for {}. Retrying after 2s (attempt {}/3)", symbol.symbol, 4 - retries);
                        counter!("symbol_sync_errors", 1, "type" => "rate_limit");
                        if retries == 0 {
                            error!("Max retries reached for {}", symbol.symbol);
                            break Profile {
                                name: None,
                                country: None,
                                ipo: None,
                                market_cap: None,
                                industry: None,
                            };
                        }
                        thread::sleep(Duration::from_secs(2));
                        retries -= 1;
                        continue;
                    }
                    counter!("symbol_sync_api_calls", 1, "endpoint" => "profile2");
                    match response.json().await {
                        Ok(profile) => break profile,
                        Err(e) => {
                            error!("Failed to parse profile for {}: {}", symbol.symbol, e);
                            counter!("symbol_sync_errors", 1, "type" => "api_parse");
                            break Profile {
                                name: None,
                                country: None,
                                ipo: None,
                                market_cap: None,
                                industry: None,
                            };
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to fetch profile for {}: {}", symbol.symbol, e);
                    counter!("symbol_sync_errors", 1, "type" => "api_fetch");
                    break Profile {
                        name: None,
                        country: None,
                        ipo: None,
                        market_cap: None,
                        industry: None,
                    };
                }
            }
        };

        let ipo_date = profile.ipo.and_then(|date: String| NaiveDate::parse_from_str(date.as_str(), "%Y-%m-%d").ok());
        let market_cap = profile.market_cap.map(|cap| (cap * 1_000_000.0) as i64);
        let record = SymbolMaster {
            symbol: symbol.symbol.clone(),
            exchange: symbol.mic.clone(),
            name: profile.name,
            sector: profile.industry.clone(),
            industry: profile.industry,
            currency: symbol.currency.clone(),
            country: profile.country,
            ipo_date,
            market_cap,
            is_active: true,
            data_source: "Finnhub".to_string(),
            last_updated: Utc::now(),
        };
        records.push(record);
    }

    // Upsert new symbols in a transaction
    let mut tx = pool.begin().await?;
    for record in &records {
        sqlx::query(r#"
            INSERT INTO symbol_master (
                symbol, exchange, name, sector, industry, currency, country, ipo_date,
                market_cap, is_active, data_source, last_updated
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (symbol) DO UPDATE SET
                exchange = EXCLUDED.exchange,
                name = EXCLUDED.name,
                sector = EXCLUDED.sector,
                industry = EXCLUDED.industry,
                currency = EXCLUDED.currency,
                country = EXCLUDED.country,
                ipo_date = EXCLUDED.ipo_date,
                market_cap = EXCLUDED.market_cap,
                is_active = EXCLUDED.is_active,
                data_source = EXCLUDED.data_source,
                last_updated = EXCLUDED.last_updated
        "#)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(&record.name)
        .bind(&record.sector)
        .bind(&record.industry)
        .bind(&record.currency)
        .bind(&record.country)
        .bind(record.ipo_date)
        .bind(record.market_cap)
        .bind(record.is_active)
        .bind(&record.data_source)
        .bind(record.last_updated)
        .execute(tx.as_mut())
        .await?;
    }
    tx.commit().await?;

    // Handle delistings
    if !delisted_symbols.is_empty() {
        sqlx::query(r#"
            UPDATE symbol_master
            SET is_active = FALSE, last_updated = $1
            WHERE symbol = ANY($2)
        "#)
        .bind(Utc::now())
        .bind(&delisted_symbols)
        .execute(&pool)
        .await?;
    }

    // Validate data
    let row = sqlx::query("SELECT COUNT(*) AS total FROM symbol_master WHERE is_active = TRUE")
        .fetch_one(&pool)
        .await?;
    let active_count: i64 = row.get("total");
    gauge!("symbol_sync_active_symbols", active_count as f64);
    if active_count < (symbols.len() as f64 * 0.9) as i64 {
        error!("Active symbols ({}) much lower than expected ({})", active_count, symbols.len());
        counter!("symbol_sync_errors", 1, "type" => "data_validation");
    }

    let start_time = chrono::Utc::now(); // or whatever value you need
    // Record job completion
    sqlx::query("INSERT INTO job_status (job_name, last_run, status, details) VALUES ($1, $2, $3, $4)")
        .bind("symbol_sync")
        .bind(start_time)
        .bind("success")
        .bind(format!("Added {} new, delisted {}", records.len(), delisted_symbols.len()))
        .execute(&pool)
        .await?;

    info!("Completed sync: {} new, {} delisted, total {}", records.len(), delisted_symbols.len(), symbols.len());
    counter!("symbol_sync_completed", 1);
    Ok(())
}
