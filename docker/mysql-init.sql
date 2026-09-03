-- Kohaku Studio 動作確認用シードデータ (MySQL)
-- 型マッピング検証のため int / bigint / float / decimal / varchar / tinyint(bool) / date / datetime を混在させる

-- 公式イメージの初期化は mysql クライアントを --default-character-set 無しで
-- 起動するため、既定(latin1)のまま日本語を流すと二重エンコードで保存される
-- (`コーヒー豆` が `ã‚³ãƒ¼ãƒ’ãƒ¼è±†` になる)。最初に接続の文字コードを揃える
SET NAMES utf8mb4;

CREATE TABLE sales (
    id          int AUTO_INCREMENT PRIMARY KEY,
    sold_on     date          NOT NULL,
    product     varchar(50)   NOT NULL,
    region      varchar(20)   NOT NULL,
    quantity    int           NOT NULL,
    unit_price  decimal(10,2) NOT NULL,
    discount    float,
    total_cents bigint        NOT NULL,
    is_member   tinyint(1)    NOT NULL,
    created_at  datetime      NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO sales (sold_on, product, region, quantity, unit_price, discount, total_cents, is_member) VALUES
    ('2026-06-01', 'コーヒー豆',  '東京', 12, 1480.00, 0.10, 1598400, 1),
    ('2026-06-01', 'ドリッパー',  '大阪',  3, 2200.00, NULL,  660000, 0),
    ('2026-06-02', 'コーヒー豆',  '東京',  8, 1480.00, 0.00, 1184000, 1),
    ('2026-06-03', 'マグカップ',  '福岡', 15,  980.00, 0.05, 1396500, 0),
    ('2026-06-04', 'コーヒー豆',  '大阪', 20, 1480.00, 0.15, 2516000, 1),
    ('2026-06-05', 'ケトル',      '東京',  2, 5800.00, NULL, 1160000, 1),
    ('2026-06-06', 'ドリッパー',  '福岡',  6, 2200.00, 0.10, 1188000, 0),
    ('2026-06-07', 'マグカップ',  '東京',  9,  980.00, NULL,  882000, 1);

CREATE TABLE customers (
    customer_id int PRIMARY KEY,
    name        varchar(50) NOT NULL,
    prefecture  varchar(20) NOT NULL,
    joined_on   date
);

INSERT INTO customers VALUES
    (1, '田中', '東京都', '2025-01-15'),
    (2, '鈴木', '大阪府', '2025-03-02'),
    (3, '佐藤', '福岡県', NULL);

-- 本物のバイナリ列 + バイナリ照合の文字列列 + 秒未満を持つ時刻(コネクタの検証用)。
-- BLOB は [BLOB] のまま残り、バイナリ照合の文字列は正しく読めること、
-- 小数秒が切り捨てられないことを確かめる
CREATE TABLE binary_types (
    id        int PRIMARY KEY,
    raw_blob  blob,
    bin_text  varchar(64) COLLATE utf8mb4_bin,
    dt        datetime(6),
    ts        timestamp(6)
);

INSERT INTO binary_types VALUES
    (1, UNHEX('89504E470D0A1A0A0000000D49484452'), '照合がバイナリの文字列',
        '2026-03-01 08:15:30.123456', '2026-03-01 08:15:30.123456'),
    (2, UNHEX('FFD8FFE000104A46494600'), 'ascii-text',
        '2026-03-01 08:15:31.000000', '2026-03-01 08:15:31.000000');
