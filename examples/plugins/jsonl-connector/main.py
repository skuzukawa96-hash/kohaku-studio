#!/usr/bin/env python3
"""Kohaku Studio コネクタプラグインのサンプル: JSON Lines (.jsonl / .ndjson) を読む。

プロトコル(docs/plugin-api-draft.md 5章):
  - 標準入力からリクエストのJSONを1行読む
  - 標準出力へレスポンスのJSONを1行書く
  - 失敗したら {"ok": false, "error": "..."} を返す(終了コードは問わない)
  - 進捗やデバッグ出力は標準エラーへ(本体がエラーメッセージに含める)

**入出力は必ずUTF-8**。Pythonの標準入出力はロケール依存(日本語Windowsでは
cp932)になるため、下の main() のように buffer 経由で明示的にUTF-8を扱う。

1リクエストごとにこのプロセスが起動されるため、状態を持つ必要はない。
"""
import json
import sys

# 型推定に使う先頭行数(本体のCSVコネクタと同じ考え方)
SAMPLE_ROWS = 2000


def read_records(path, max_rows):
    """JSONL を読み、辞書のリストと列名の出現順リストを返す"""
    records = []
    columns = []
    seen = set()
    with open(path, "r", encoding="utf-8") as f:
        for lineno, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue  # 空行は読み飛ばす
            if max_rows is not None and len(records) >= max_rows:
                break
            try:
                obj = json.loads(line)
            except json.JSONDecodeError as e:
                raise ValueError("%d行目のJSONが不正です: %s" % (lineno, e.msg))
            if not isinstance(obj, dict):
                raise ValueError("%d行目がオブジェクトではありません" % lineno)
            records.append(obj)
            for k in obj:
                if k not in seen:
                    seen.add(k)
                    columns.append(k)
    return records, columns


def infer_type(records, col):
    """列の型を推定する。本体の型名(integer / real / boolean / text)を返す"""
    seen_int = seen_float = seen_bool = seen_other = False
    for r in records[:SAMPLE_ROWS]:
        v = r.get(col)
        if v is None:
            continue
        if isinstance(v, bool):
            seen_bool = True
        elif isinstance(v, int):
            seen_int = True
        elif isinstance(v, float):
            seen_float = True
        else:
            seen_other = True
    if seen_other or (seen_bool and (seen_int or seen_float)):
        return "text"
    if seen_bool:
        return "boolean"
    if seen_float:
        return "real"
    if seen_int:
        return "integer"
    return "text"


def cell(v):
    """表に収まらない値(配列・オブジェクト)はJSON文字列にして落とさない"""
    if isinstance(v, (dict, list)):
        return json.dumps(v, ensure_ascii=False)
    return v


def handle(req):
    cmd = req.get("cmd")

    if cmd == "describe":
        return {
            "ok": True,
            "name": "jsonl-connector",
            "extensions": ["jsonl", "ndjson"],
            "schemes": [],
        }

    if cmd == "list_objects":
        # 1ファイル=1テーブルなので、ファイル名(拡張子なし)を1つ返す
        path = req.get("path", "")
        stem = path.replace("\\", "/").rsplit("/", 1)[-1]
        if "." in stem:
            stem = stem.rsplit(".", 1)[0]
        return {"ok": True, "objects": [stem or "data"]}

    if cmd == "load":
        path = req.get("path", "")
        options = req.get("options") or {}
        max_rows = options.get("max_rows")
        records, columns = read_records(path, max_rows)
        if not columns:
            return {"ok": False, "error": "データがありません"}
        types = {c: infer_type(records, c) for c in columns}
        return {
            "ok": True,
            "table": {
                "columns": [{"name": c, "type": types[c]} for c in columns],
                # 欠けているキーは None(=NULL)で埋め、列数を揃える
                "rows": [[cell(r.get(c)) for c in columns] for r in records],
            },
        }

    return {"ok": False, "error": "未知のコマンドです: %s" % cmd}


def main():
    # ロケールに左右されないよう、バイト列として読み書きしてUTF-8を明示する
    line = sys.stdin.buffer.readline().decode("utf-8", errors="replace")
    try:
        req = json.loads(line) if line.strip() else {}
        resp = handle(req)
    except FileNotFoundError as e:
        resp = {"ok": False, "error": "ファイルを開けません: %s" % e}
    except Exception as e:  # プラグイン側の例外は必ずJSONにして返す
        resp = {"ok": False, "error": "%s: %s" % (type(e).__name__, e)}
    out = json.dumps(resp, ensure_ascii=False) + "\n"
    sys.stdout.buffer.write(out.encode("utf-8"))
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()
