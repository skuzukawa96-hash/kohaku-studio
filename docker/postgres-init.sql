-- Kohaku Studio 動作確認用シードデータ (PostgreSQL)
-- 型マッピング検証のため int / bigint / real / numeric / text / bool / date / timestamp を混在させる

CREATE TABLE sales (
    id          serial PRIMARY KEY,
    sold_on     date        NOT NULL,
    product     text        NOT NULL,
    region      varchar(20) NOT NULL,
    quantity    integer     NOT NULL,
    unit_price  numeric(10, 2) NOT NULL,
    discount    real,
    total_cents bigint      NOT NULL,
    is_member   boolean     NOT NULL,
    created_at  timestamp   NOT NULL DEFAULT now()
);

INSERT INTO sales (sold_on, product, region, quantity, unit_price, discount, total_cents, is_member) VALUES
    ('2026-06-01', 'コーヒー豆',   '東京', 12, 1480.00, 0.10, 1598400, true),
    ('2026-06-01', 'ドリッパー',   '大阪',  3, 2200.00, NULL,  660000, false),
    ('2026-06-02', 'コーヒー豆',   '東京',  8, 1480.00, 0.00, 1184000, true),
    ('2026-06-03', 'マグカップ',   '福岡', 15,  980.00, 0.05, 1396500, false),
    ('2026-06-04', 'コーヒー豆',   '大阪', 20, 1480.00, 0.15, 2516000, true),
    ('2026-06-05', 'ケトル',       '東京',  2, 5800.00, NULL, 1160000, true),
    ('2026-06-06', 'ドリッパー',   '福岡',  6, 2200.00, 0.10, 1188000, false),
    ('2026-06-07', 'マグカップ',   '東京',  9,  980.00, NULL,  882000, true);

CREATE TABLE customers (
    customer_id integer PRIMARY KEY,
    name        text NOT NULL,
    prefecture  text NOT NULL,
    joined_on   date
);

INSERT INTO customers VALUES
    (1, '田中', '東京都', '2025-01-15'),
    (2, '鈴木', '大阪府', '2025-03-02'),
    (3, '佐藤', '福岡県', NULL);

-- public 以外のスキーマ + 秒未満を持つ時刻(コネクタの検証用)。
-- 一覧に schema.table の形で出て、読み込めること・小数秒が残ることを確かめる
CREATE SCHEMA fab;

CREATE TABLE fab.events (
    id     integer     PRIMARY KEY,
    ts     timestamp   NOT NULL,
    tstz   timestamptz NOT NULL
);

INSERT INTO fab.events VALUES
    (1, '2026-03-01 08:15:30.123456', '2026-03-01 08:15:30.123456+09'),
    (2, '2026-03-01 08:15:31',        '2026-03-01 08:15:31+09');
