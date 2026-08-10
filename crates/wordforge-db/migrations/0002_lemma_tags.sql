-- 詞條標籤：考試範圍（zk 國中會考 / gk 學測 / cet4 / ielts…）與常用度（oxford3000、collins5）。
--
-- 存成空白分隔的字串而不是關聯表：標籤數量少、幾乎只用來篩選與顯示，
-- 多一張表要多一次 JOIN，換來的正規化在這裡沒有實際好處。
-- 查詢時用 ' ' || tags || ' ' LIKE '% zk %' 避免 zk 誤中 zkk。
ALTER TABLE lemma ADD COLUMN tags TEXT NOT NULL DEFAULT '';
