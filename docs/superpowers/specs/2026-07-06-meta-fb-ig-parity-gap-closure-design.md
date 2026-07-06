# Meta 頻道(Facebook Messenger + Instagram DM）— LINE 同級「GAP 收尾」設計

**日期**:2026-07-06
**狀態**:草案(待審核 → 通過後才實作）
**類型**:backend adapter gap-closure（Rust axum + sqlx/Postgres)

---

## 0. 前情更正（重要 — 為何這是「收尾」而非「從零打造」)

原始任務假設「Facebook 目前只有 webhook 簽章驗證 + read-receipt watermark，把 FB/IG
一起補到與 LINE 同級」。**核實後此前提已過期**:直接讀碼 + 跑既有測試證實 Facebook
與 Instagram 的核心鏈路**已實作、已接通、且有測試通過**。

### 已完成且驗證(讀碼 + 15 個測試通過)

**Inbound(FB + IG 共用)**
- 路由 `POST /api/webhooks/facebook`(GET handshake + 任意方法),FB/IG 共用同一端點
  （`webhooks/mod.rs:30`)。IG 靠 payload `object="instagram"` 分流,不需獨立路由。
- Handshake(`hub.mode/verify_token/challenge`)+ `x-hub-signature-256` HMAC-SHA256（`sha256=` hex）
  驗簽,secret = `config.facebook_app_secret`（`handlers.rs:520-604`）。
- `object=page→facebook` / `object=instagram→instagram` 分派 → `process_messaging_item`
  （`handlers.rs:433-516`),涵蓋:
  - message/attachment → `normalize_facebook` / `normalize_instagram`（`parse.rs:168-298`）
  - postback → `normalize_facebook_postback`(以 `meta_postback_event_key` 去重）
  - delivery → `mark_delivered`;read → `mark_read`(watermark)/`mark_read_by_mid`
  - reaction → `apply_reaction`
  - **echo(`is_echo`/`is_self`)→ 跳過**（`handlers.rs:444-454`)— 任務點名的「Meta 特有坑」已處理
  - IG **story mention / story reply** → 標記入庫（`parse.rs:271-298`,`storyMention`/`storyReply` metadata）
- 冪等:`messages.platform_message_id` unique（`0009_channels.sql`）+ `webhook_replay_events`
  （`0017`)分別對 message / 生命週期事件去重。
- 完整 ingest：customer upsert → conversation upsert → message insert → 即時推送
  `new_message`（`ingest.rs::ingest_message` + `spawn_followups`）。

**Outbound**
- `send_batch` 分派 `"facebook"→fb_send` / `"instagram"→fb_send`(IG 重用 FB Send API,
  `channels.rs:548-576`);`fb_send` 真的 POST `graph.facebook.com/v21.0/me/messages`
  （`channels.rs:288-324`)。**文字發送可用**。

**Profile**:`meta_profile`（`?fields=name,username,profile_pic`,含 IG 欄位,`channels.rs:460-462`）。

**基礎設施**:`resolve_channel` 已處理 FB(`accessToken`/`appSecret`)+ IG(`accessToken`)
（`channels/resolve.rs:36-50`);config 已有 `FACEBOOK_APP_SECRET`/`FACEBOOK_VERIFY_TOKEN`/
`FACEBOOK_PAGE_ACCESS_TOKEN`/`INSTAGRAM_ACCESS_TOKEN`/`META_GRAPH_URL`;
`channel_integrations` platform_fields 已含 FB(`pageId`)+ IG(`igId`);credentials 走
`crypto::protect` 加密。

**測試**（本次執行,全過）:`backend/tests/webhooks.rs` — 9 FB + 6 IG
（handshake / 簽章 / echo skip / postback / delivery / read watermark / oversize;
IG message / echo / reaction / unreaction / read-by-mid / **story mention**)。

> 因此本設計的範圍是**補齊剩餘 GAP 到與 LINE 同級**,不重寫已存在的鏈路。

---

## 1. 目標與範圍

**目標**:把 FB/IG 從「文字級可用」補到「與 LINE 同級」——原生媒體收發、
明確的政策錯誤呈現、憑證健康度可見。

**範圍(依優先序)**

| # | GAP | 優先 |
|---|-----|------|
| G1 | Outbound 原生媒體(FB/IG 發送 image/video/audio/file 走 Meta 原生 attachment,而非文字連結) | P1 |
| G2 | Inbound 媒體顯示(FB/IG 收到的媒體目前顯示壞掉——代理/鏡像僅支援 LINE) | P1 |
| G3 | 24 小時回覆窗錯誤 UX(超窗被 Meta 拒絕時,給客服明確訊息而非 generic error) | P1 |
| G4 | Token 過期偵測 + `channel_integrations.last_error` 記錄 | P2 |
| G5 | HUMAN_AGENT tag(設計預留、預設不用,7 天窗) | P2 |
| G6 | IG 憑證 verify(`verify_meta_node` 目前只做 FB) | P3 |
| G7 | Graph API 版本一致性(硬編 v21.0 vs `config.meta_graph_url` v20.0) | P3 |
| G8 | 「共用 Meta core」抽象化評估(現況已相當 DRY,可選) | P3 |

**非目標**:重寫既有 inbound/outbound 鏈路;WhatsApp;新增前端功能（前端多平台
渲染已就緒,G2 以後端補齊為主）。

---

## 2. 已確認事實（任務要求「先確認」的項目)

- **平台識別字串**:DB `platform` 用 `'facebook'` / `'instagram'`(對齊前端
  `channels.ts:22-26` glyph:`facebook→'fb'`、`instagram→'ig'`)。既有碼已如此,不變。
- **Secret 共用**:IG 與 FB **共用同一個 Meta app secret**——`facebook_webhook` 對 FB/IG
  皆用 `config.facebook_app_secret` 驗簽(`handlers.rs:575`),**不需要新環境變數**。
  outbound token 則各自解析(`resolve_channel`:FB `accessToken`、IG `accessToken`,
  可 fallback `facebook_page_access_token`,`resolve.rs:55-66`)。
- **冪等**:沿用 `messages.platform_message_id` unique + `webhook_replay_events`;IG 的
  `mid` 同樣走這條(`process_messaging_item` 用 `message.mid`),不需新機制。
- **Echo**:已處理(見 §0),本設計不動,但 G1 送出時**必須確保**送出的訊息之後
  以 `is_echo` 回流時不重複入庫——現況 echo 直接跳過即滿足,G1 不改變此性質。

---

## 3. 各 GAP 設計

### G1 — Outbound 原生媒體（P1）

**現況**:`fb_send`（`channels.rs:288-324`)對 `it.media` 只做
`format!("📎 {name}\n{url}")` 當文字送(line 292-295)。LINE 則以
`line_message()` 送原生 image/video/audio(`channels.rs:87-112`)。

**設計**:讓 `fb_send` 依 `OutboundMedia.kind` 建 Meta 原生 attachment body:
```jsonc
// image / video / audio / file 通用
{
  "recipient": { "id": <PSID/IGSID> },
  "messaging_type": "RESPONSE",
  "message": {
    "attachment": {
      "type": "image" | "video" | "audio" | "file",
      "payload": { "url": <signed_public_url>, "is_reusable": false }
    }
  }
}
```
- `OutboundMedia.kind`(`MediaKind::Image/Video/Audio/File`)→ Meta `type`。
- `url` 沿用既有 outbound 簽章公開網址(`signed_public_url`,7 天 TTL,
  `conversations/handlers.rs:937-941`),與 LINE image 同一套。
- 新增 `fb_send_attachment_body(recipient, kind, url)`,與既有 `fb_send_body`
  （純文字）並存;`fb_send` 依 `it.media.is_some()` 擇一。
- **IG media 型別限制差異**:IG DM 送出僅穩定支援 image(audio/video/file 支援度受限)。
  設計:IG 平台送非 image 媒體時,fallback 成「文字 + 連結」(保留現況行為當退路),
  並在 metadata 記 `mediaFallback: true` 供診斷;FB 則全型別走原生。此差異以
  `platform` 參數在 `fb_send` 內分支(是唯一的 FB/IG 送出差異點)。
- **無 schema 變更**。

**測試**:mock Graph API(既有測試如何 mock 見 §5)→ 送 image 應產生
`message.attachment.type=image`;IG 送 video 應 fallback 成文字連結。

---

### G2 — Inbound 媒體顯示（P1）

**現況(壞的)**:
- FB/IG inbound 媒體存 `metadata.media = { type, contentUrl }`,`contentUrl` 是
  **Meta CDN 直連網址**(`parse.rs:219`)。
- 前端 inbound 媒體一律走代理 `GET /api/conversations/{id}/messages/{msgId}/media`
  （`MessageMedia.tsx:53`,inbound 無 `srcUrl`)。
- 代理 `proxy_media_inner`（`conversations/handlers.rs:660-715`)**硬編 LINE**:
  `resolve_channel("line")` + `fetch_line_media(token, platform_message_id, …)`,用 LINE
  content-id API。FB/IG 的 `platform_message_id` 是 Meta `mid`,打 LINE API → 失敗。
- 背景媒體鏡像 `process_media`（`queue/worker.rs:254-353`)同樣 LINE-only
  （`fetch_line_media_from_base` + key `line/media/{id}`),FB/IG 雖 enqueue 但下載失敗。

**設計(推薦:擴充「鏡像至 ingest」,平台感知)**
Meta CDN 網址會過期,view-time 才代理有失效風險 → 採**入庫時鏡像**(捕捉即時有效
網址),重用既有 media queue + asset 基礎:
1. **enqueue 帶來源**:ingest 對 Meta 媒體 enqueue 時,除 `platformMessageId` 另帶
   `platform` 與 `sourceUrl = media.contentUrl`（`ingest.rs:717-732` 附近)。
2. **`process_media` 平台感知**（`worker.rs:254`):
   - `platform == "line"` → 現況 `fetch_line_media_from_base(...)`(不變)。
   - `platform ∈ {facebook, instagram}` → 從 `sourceUrl` 直接下載 bytes(新增
     `fetch_url_media(url) -> Option<(bytes, content_type)>`,無 bearer),存 asset
     key `meta/media/{platform_message_id}`。
   - 下載成功後 asset 入庫方式與 LINE 一致(files/assets 表,帶 `platform`、`file_type`)。
3. **代理 `proxy_media_inner` 平台感知**:讀 message 時取其 `platform`;Meta 訊息改由
   已鏡像的 asset 提供(若鏡像完成)或 fallback 直接串流 `sourceUrl`(未鏡像時的暫時退路)。
   前端不需改(仍打同一代理 URL)。
4. **非可下載型別**(location/sticker/story)維持現況文字化,不進媒體流程。

**替代方案(較簡單但有失效風險)**:純 view-time——`proxy_media_inner` 對 Meta 訊息
直接串流 `metadata.media.contentUrl`。優點:改動小、不動 queue;缺點:Meta URL 過期後
404。**建議採鏡像方案**(對齊 LINE 的既有取捨),spec 審核時可否決改用簡易版。

**無 schema 變更**(重用 files/assets + media queue)。

**測試**:webhook 灌一則 FB image → 斷言 message `content_type=image` 且 media
metadata 有 `contentUrl`;queue 處理後 asset 存在;代理回 200 + 正確 content-type
（Meta 下載以測試替身 mock)。

---

### G3 — 24 小時回覆窗錯誤 UX（P1）

**現況**:`fb_send` 失敗 → `OutboundError::PlatformRejected { status, body }`
（`channels.rs:308-315`)→ `deliver_pending` 存 `delivery_status=failed` + 廣播
`error = e.to_string()`(`channels.rs:657-717`)。客服看到的是 generic 失敗字串。

**設計**:解析 Meta 錯誤 JSON,映射成明確、可行動的訊息。
- Meta 錯誤 body 形如 `{"error":{"message":..,"code":10,"error_subcode":2018278,"type":"OAuthException"}}`。
- 新增 `classify_meta_error(status, body) -> MetaSendError`,辨識:
  - **超窗**:`code=10`（或 subcode 對應 messaging-window)→ 使用者訊息
    「超出 24 小時客服回覆窗,客戶需再次來訊後才能回覆(或使用已核准的訊息標籤)」。
  - **Token 失效**:`code=190`（OAuthException)→ 見 G4。
  - 其他 → 保留原 message。
- 呈現路徑:`deliver_pending` 廣播的 `error` 欄改帶**已映射的中文訊息**
  （前端已顯示 delivery error,不需改前端;若前端目前未顯著呈現,spec 附註但不擴大範圍）。
- 訊息文案集中於一處常數表,便於維護。

**測試**:mock Graph 回 `code=10` → 斷言廣播 error 含「24 小時」字樣、`delivery_status=failed`。

---

### G4 — Token 過期偵測 + `last_error` 記錄（P2）

**現況**:send 失敗只進 message 的 `delivery_status/error` 廣播,不寫回
`channel_integrations`。該表已有 `last_error TEXT`(JSON)、`error_count`、`is_verified`
（`0001_init.sql`)。

**設計**:
- `classify_meta_error`(見 G3)辨識 `code=190` OAuthException → 判為 token 失效。
- 於 outbound 失敗路徑,對該 integration 寫 `last_error = {timestamp, type:"token_expired",
  message, context}`、`error_count += 1`、`is_verified = false`(BIGINT 0)。沿用既有
  channels store 的 error 寫入慣例(對齊 verify 失敗的寫法)。
- 前端頻道管理頁已顯示 `last_error`,故 token 過期會自然浮現給管理員。

**無 schema 變更**。**測試**:mock `code=190` → 斷言 integration `last_error.type=token_expired`。

---

### G5 — HUMAN_AGENT tag（P2,設計預留、預設不用）

**背景**:Meta 允許以 `messaging_type=MESSAGE_TAG` + `tag=HUMAN_AGENT` 把回覆窗延長到 7 天
（需申請 Human Agent 權限)。

**設計**:
- `fb_send_body` / attachment body 增加可選的 `messaging_type` + `tag`;預設維持
  `RESPONSE`(24h 窗)。
- 由 config 旗標控制(如 `META_HUMAN_AGENT_TAG=true`,預設 false);開啟時超窗改用
  `MESSAGE_TAG/HUMAN_AGENT`。**預設關閉,不改變現行行為**——僅預留擴充點與程式碼路徑。
- 文件註明:實際啟用需先在 Meta 後台取得權限,否則 API 仍會拒絕。

**測試**:旗標關(預設)→ body 為 `RESPONSE`;旗標開 → body 帶 `MESSAGE_TAG/HUMAN_AGENT`。

---

### G6 — IG 憑證 verify（P3）

**現況**:`verify_meta_node` 做 FB(`{graph}/{pageId}?fields=id,name`),IG 無 verify。
**設計**:IG verify 打 `{graph}/{igId}?fields=id,username`,沿用 `verify_meta_node` 形態,
成功寫 `is_verified=1`/`verified_at`。**測試**:mock Graph 回 `{id,username}` → verify 成功。

---

### G7 — Graph API 版本一致性（P3 清理）

**現況**:`fb_send`/`meta_profile` 硬編 `v21.0`;`config.meta_graph_url` 預設 `v20.0` 未被用。
**設計**:統一改讀 `config.meta_graph_url`(單一真相源),移除硬編版本;預設值升為 `v21.0`
以維持現行外部行為。**測試**:既有 FB/IG 測試不變即通過(行為等價)。

---

### G8 — 「共用 Meta core」抽象評估（P3,可選）

**現況**:已相當 DRY——IG 送出重用 `fb_send`、`normalize_instagram` 委派
`normalize_facebook`、FB/IG 共用一個 webhook 端點與驗簽。任務要求的「core + 薄平台層」
架構**基本已達成**。
**設計**:僅在 G1–G4 落地過程中若發現 FB/IG 分支重複(如媒體型別限制),就地把差異收斂為
小的 `platform`-參數化函式即可,**不做大重構**(YAGNI)。本項無獨立產出,是實作時的紀律。

---

## 4. 資料模型

**不需新資料表**。全部重用既有:`messages`(`platform_message_id`、`metadata.media`、
`content_type`、`read_at`)、`channel_integrations`(`last_error`/`error_count`/`is_verified`)、
files/assets(媒體鏡像)、`webhook_replay_events`。
> 若審核時決定 G2 改採「新 media_assets 表」等方案,新表一律 TIMESTAMPTZ / BOOLEAN / JSONB。

---

## 5. 測試計畫

沿用 `backend/tests/webhooks.rs`(inbound)與 `backend/tests/channels.rs`(outbound)既有寫法:
- **Inbound**:`spawn_app` + 對 `/api/webhooks/facebook` POST 帶正確 `x-hub-signature-256`
  （以測試 secret 算 HMAC),斷言 DB 落地(既有 15 個測試即此模式)。G2 再加 FB/IG media
  ingest + 代理回傳測試。
- **Outbound**:mock Graph API(既有 outbound 測試如何攔截 HTTP——實作前先確認 `channels.rs`
  測試用的 http 替身機制,以此為準),G1/G3/G4/G5/G6 各加一組:原生 image body、IG fallback、
  `code=10` 超窗訊息、`code=190` token 過期寫 `last_error`、HUMAN_AGENT 旗標、IG verify。
- **簽章**:兩平台各一套(FB/IG 共用 secret,故一組 helper 即可)。

**每個 GAP 各帶整合測試**,實作順序見 §6。

---

## 6. 實作順序（每步帶測試 + 遵守 CLAUDE.md)

1. **G7**(版本統一,最小、無行為變更)+ **G6**(IG verify)——暖身、低風險。
2. **G1**(outbound 原生媒體)+ 其測試。
3. **G3**(24h 窗錯誤映射)——與 G1 同區(`fb_send`/`deliver_pending`),一起做。
4. **G4**(token 過期 + last_error)——重用 G3 的 `classify_meta_error`。
5. **G2**(inbound 媒體鏡像 + 代理平台感知)——較大,獨立一步。
6. **G5**(HUMAN_AGENT 旗標,預設關)——收尾。

每步:改 symbol 前 GitNexus `impact`;commit 前 `detect_changes()`;本地過
`cargo fmt`/`clippy -D warnings`/`audit`。

---

## 7. 約束

- **CLAUDE.md**:改 symbol 前 `impact`,commit 前 `detect_changes()`,HIGH/CRITICAL 先示警。
- **Secret**:全走環境變數(FB/IG 共用 `FACEBOOK_APP_SECRET`,不新增變數);DB credentials
  必經 `crypto::protect`。
- **冪等**:沿用 `webhook_replay_events` + `platform_message_id` unique(IG mid 適用)。
- **Echo**:維持現行 `is_echo`/`is_self` 跳過,G1 送出不得造成回流重複入庫。
- **新表(如有)**:TIMESTAMPTZ / BOOLEAN / JSONB。
- **CI**:`cargo fmt`/`clippy`/`audit` 本地先過。

---

## 8. 待審核決策（請你定奪)

1. **範圍**:是否納入全部 G1–G7(P1–P3),或先只做 P1(G1/G2/G3)？(你先前選「GAP 收尾」,
   此 spec 以全 GAP 呈現,實作可分批。)
2. **G2 方案**:採「入庫鏡像」(推薦,對齊 LINE、抗 Meta URL 過期)還是「view-time 直接串流
   contentUrl」(改動更小、但 URL 過期會失效)？
3. **G5 HUMAN_AGENT**:確認「預設關、僅預留」符合你的意思(不預設啟用)。
4. **前端**:G3 錯誤訊息若前端目前呈現不明顯,是否允許最小前端調整(顯示 delivery error 文案)?
   （任務原則上不動前端,故預設只改後端廣播的 error 文案。)

> **本 spec 寫完即停,待你審核通過後才依 §6 實作。**
