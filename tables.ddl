CREATE TABLE IF NOT EXISTS symbol_master (
    id SERIAL PRIMARY KEY,
    symbol TEXT NOT NULL UNIQUE,
    exchange TEXT,
    name TEXT,
    sector TEXT,
    industry TEXT,
    currency TEXT,
    country TEXT,
    ipo_date DATE,
    market_cap BIGINT,
    is_active BOOLEAN DEFAULT TRUE,
    data_source TEXT DEFAULT 'Finnhub',
    last_updated TIMESTAMP DEFAULT now()
);