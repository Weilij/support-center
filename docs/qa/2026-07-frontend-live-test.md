# 前端實測驗證報告 — 2026-07-06

近三輪前端大改（glass/RBAC 重構、健檢優化、UX 優化，~20+ commits）的人肉 QA 自動化版。
**瀏覽器實走**（Chrome DevTools MCP），非跑單元測試。發現的問題只記錄不修。

> **狀態更新（2026-07-06，本報告產出後同日）**：本報告的 **F1／F2／F3 皆已修復**，
> 另補修一項本報告未涵蓋的 **F4**（登入回應未含 teams，剛登入的 session 缺團隊
> 脈絡）。以下內文保留當時的原始觀察，未回溯改寫；各發現段落已標註修復 commit。
>
> | 發現 | 狀態 | 修復 commit |
> |------|------|-------------|
> | F1 seed `conversations.updated_at` | ✅ 已修 | `c5dea43`（採建議修法①：seed 補欄，未動 schema） |
> | F2 `/me` teams[] 缺團隊名 | ✅ 已修 | `b97ad9c` |
> | F3 客戶面板團隊標示未即時刷新 | ✅ 已修 | `c2cc6cc` |
> | F4 登入回應未含 teams（本報告未涵蓋） | ✅ 已修 | `bc9932d` |
>
> 截圖 `screens/15-fixes-f2-f3-verified.png` 為 F2/F3 修復後的驗證畫面。

## 環境

| 項目 | 值 |
|------|-----|
| DB | Postgres 18（docker `mcss-qa-db`，host `:5433`，避開本機原生 pg 佔用的 5432） |
| 後端 | `cargo run`（debug）`:3000`，`ENVIRONMENT=development` |
| 前端 | `npm run dev`（vite）`:5174`（5173 被 Chrome 佔用；vite proxy `/api`→`:3000`） |
| 種子 | `cargo run --example seed` → `admin@example.com / admin123`、團隊「客服一組」、2 示範對話 |
| 額外造帳 | `agent@example.com / admin123`（agent 角色）；第二團隊「客服二組」（供轉移測試） |

> **瀏覽器工具限制**：MCP 的原生 `click` 不會觸發 React Router 的 SPA 導航（實測側欄連結、頭像連結原生 click 皆無反應）；改用 `eval` 的 `element.click()` 可正常導航。受控 input 亦需用 React 的 native value setter + `input` 事件才會進 state。以上為**工具限制，非 app bug**。

---

## 測試結果總覽

| # | 流程 | 結果 | 截圖 |
|---|------|------|------|
| 1 | 登入分流 + Profile 入口 | ✅ PASS | 01, 02, 09 |
| 2 | 收件匣核心流（發文字/圖片、Enter/Shift+Enter/IME、樂觀更新） | ✅ PASS（2 項僅代碼驗證） | 03, 04, 05 |
| 3 | 草稿跨對話保留 | ✅ PASS | — |
| 4 | inline 指派/轉移/取消/快捷 | ✅ PASS（含 2 minor 發現） | 06 |
| 5 | 未讀與即時進線 | ✅ PASS（音效/桌面通知僅代碼驗證） | 14 |
| 6 | 斷線 banner + 送出 disabled | ✅ PASS | 07 |
| 7 | 通知深連結對話 | ✅ PASS | — |
| 8 | 廣播確認 + 空欄擋 | ✅ PASS | 10 |
| 9 | 深色模式（MetricsView/DataTable/錯誤文字） | ✅ PASS | 11, 12, 13 |
| 10 | 載入與錯誤 + 重試 | ✅ PASS | 08 |
| 11 | 鍵盤（Teams button、Modal focus trap、Esc） | ✅ PASS | — |
| — | **環境/種子** | 🔴 **FAIL（seed 壞掉，見 F1）** | — |

**11 個功能流程全 PASS**；發現 **1 個真 bug（種子腳本）** + **2 個 minor（顯示）**。

---

## 各流程細節

### 1. 登入分流 + Profile 入口 — ✅ PASS
- `agent@example.com` 登入 → 落 `/conversations`（標題「對話收件匣」）。截圖 01。
- `admin@example.com` 登入 → 落 `/dashboard`（標題「儀表板」，側欄含日常/營運/分析/系統全群組）。截圖 09。
- topbar 頭像/名字為 `<a href="/profile" aria-label="個人資料">`，點擊進 `/profile`，含「通知偏好」卡（音效 checkbox + 桌面通知授權鈕）。截圖 02。
- agent 側欄僅「日常」群組（RBAC 正確）。

### 2. 收件匣核心流 — ✅ PASS（2 項僅代碼驗證）
- 開對話：URL 深連結更新（`/conversations/{id}`），訊息載入，開啟後該對話未讀 badge 清零、其他對話 badge 保留。截圖 03。
- **Enter 送出**：✅ 訊息「您好，營業時間是週一至週五 9:00-18:00」送出、composer 清空、顯示「已讀」。
- **Shift+Enter**：✅ 不送出、草稿保留（composer 仍含文字）。
- **IME 選字 Enter（`isComposing:true`）**：✅ 不誤送、草稿保留。
- **發圖片（選檔）**：✅ 附件預覽 chip（縮圖 + `qa-test.png ×`）→ 送出 → thread 顯示 image bubble、已讀。截圖 04, 05。
- **樂觀更新**：訊息送出後立即顯示並確認為「已讀」——功能正常；惟 pending 半透明 → 確認的**過渡瞬間太快，未截到**（非缺陷）。
- **拖放 / 貼上圖片**：未經瀏覽器自動化驗證（CDP 難可靠模擬 drag/paste 檔案）；三種入口共用同一 `addFiles` handler，選檔路徑已驗證。

### 3. 草稿跨對話保留 — ✅ PASS
在王小明打「這是王小明的草稿測試」→ 切陳美玲（composer 空、無殘留）→ 切回王小明（草稿還原）。

### 4. inline 指派/轉移 — ✅ PASS（含 2 minor）
- header「指派團隊」鈕 → inline dropdown：`指派給團隊 / ✓ 客服一組 目前 / 客服二組 / 取消指派 / 填寫原因…`。截圖 06。
- **轉移**（填原因「VIP 客戶，轉專責團隊」→ 點客服二組）：Toast「已指派給「客服二組」」、選單關閉、**後端 DB team_id=2 已持久化**（重載後客戶面板正確顯示客服二組）。
- **取消指派**：Toast「已取消指派」。
- **快捷「指給我的團隊」**：Toast 出現（但見 **F2**）。
- **📌 F3（minor）**：轉移/指派後，右側客戶面板「團隊」標示**未即時刷新**（仍顯示舊團隊，重載才更新）。
- **📌 F2（minor）**：快捷 Toast 顯示「已指派給「**Team 1**」」而非「客服一組」（見下方發現）。

### 5. 未讀與即時進線 — ✅ PASS（音效/桌面通知僅代碼驗證）
- 以測試 secret 對 `/api/webhook` 送簽章正確的 LINE inbound（王小明 U-demo-1「請問可以退貨嗎？」，HTTP 200，訊息入庫為 customer）。
- 瀏覽器收件匣**即時**（無重載）：王小明**置頂**、預覽更新為新訊息、未讀 badge「1」出現。截圖 14。
- 開啟王小明 → badge 清零、新訊息入 thread。
- **音效提示 + 瀏覽器桌面通知**：headless 環境無法驗證（音訊輸出 / OS 級 Notification / 需分頁背景 + 授權）；邏輯由 `incomingAlerts.test`（4 tests）覆蓋：跳過自己的訊息、跳過正在看的對話、背景才發桌面通知。

### 6. 斷線 banner — ✅ PASS
- kill 後端 → WS 關閉 → banner「連線中斷，正在重新連線…」出現、送出鈕 `disabled`、composer 顯示斷線提示。截圖 07。
- 重啟後端 → WS 重連 → banner 消失（恢復正常）。「連線已恢復」過渡（2.5s）未截到，但斷線→恢復消失行為確認，過渡由單元測試覆蓋。

### 7. 通知深連結 — ✅ PASS
插入帶 `data.conversationId` 的對話通知 → `/notifications` 顯示「王小明 傳來新訊息 ❗· 查看對話 →」→ 點擊跳 `/conversations/b004ef19...`（正確對話）。

### 8. 廣播確認 — ✅ PASS
- 空欄位時「發送廣播」鈕 `disabled`（空欄擋）。
- 填標題+內容 → 鈕啟用 → 點擊 → 彈 ConfirmDialog「確定要將這則廣播發送給所有使用者嗎？「系統維護通知」將立即送達每一位使用者的通知中心。」截圖 10。

### 9. 深色模式 — ✅ PASS
- 全站預設深色（跟隨系統）。掃描各頁**無不可讀區塊**。
- **SystemMonitoring / MetricsView**（審查點名「寫死 #444/#888/#eee」處）：label/值/邊框皆改用 `--ink-2/--muted/--line/--ink` token，深色完全可讀。截圖 12。
- **DataTable**（/agents）：表頭、列、空狀態皆 token 化可讀。截圖 13。
- 錯誤文字：`--color-danger` token 於深色可讀（見流程 10）。
- Teams 頁深色 + 功能正常。截圖 11。

### 10. 載入與錯誤 + 重試 — ✅ PASS
- 停後端 → SPA 內導向 `/dashboard` → 顯示「伺服器發生錯誤，請稍後再試」+「重試」鈕（`ErrorRetry`，**非白屏**）。截圖 08。
- 重啟後端 → 點「重試」→ 儀表板恢復（對話總數 2、訊息總數 4、客戶總數 2、渠道分佈…）。
- 註：整頁 reload（非 SPA 導航）時後端若停，`/me` 失敗會導向 /login（正常安全行為），故錯誤態須以 SPA 內導航觀察。

### 11. 鍵盤可及性 — ✅ PASS
- **Teams 團隊項目**：由 `<span onClick>` 改為真 `<button>`（Phase 3.5），可 focus、原生鍵盤可操作（「客服一組（2）」）。
- **Modal focus trap**：ConfirmDialog 開啟時 focus 自動進入對話框（落在第一個可互動元素「取消」鈕）；按 **Esc 關閉**對話框。

---

## 發現（Findings）

### 🔴 F1 — `examples/seed.rs` 種子腳本壞掉（真 bug）— ✅ 已修（`c5dea43`）
**嚴重度：中（僅影響開發/示範環境，不影響生產；但阻斷新環境 bring-up）**

**現象**：`cargo run --example seed` 失敗：
```
null value in column "updated_at" of relation "conversations" violates not-null constraint
```
**根因**：Phase 2 schema 優化的 **migration 0021** 將 `conversations.updated_at` 設為 `NOT NULL` 且**未加 DEFAULT**。`examples/seed.rs:72` 的 conversation INSERT 欄位清單為
`(id, customer_id, team_id, status, priority, last_message_at, created_at)` — **省略了 `updated_at`** → 插入 NULL → 違反約束。
**為何沒被 CI 抓到**：生產路徑（`webhooks/ingest.rs:355`、`liff/handlers.rs:418`）的 conversation INSERT **有**明確寫 `updated_at`，所以 app 正常；而 CI 不會跑 seed example，故漏網。
**重現**：乾淨 DB → `cargo run --example seed`。
**本次解阻手法（僅 runtime，未改碼）**：`ALTER TABLE conversations ALTER COLUMN updated_at SET DEFAULT now();` 後 seed 通過。
**建議修法（擇一）**：① seed 的 INSERT 補上 `updated_at`（對稱 created_at）；② 或在 migration 0021 為該欄加 `DEFAULT now()`（同時也讓其他省略該欄的插入更健壯）。

### 🟡 F2 — 快捷指派 Toast 顯示「Team 1」而非團隊名（顯示不一致）— ✅ 已修（`b97ad9c`，另見 F4 `bc9932d`）
**嚴重度：低（純顯示，功能正常）**

**現象**：composer「指給我的團隊」快捷鈕的 Toast 顯示「已指派給「**Team 1**」」，但 header 指派下拉正確顯示「客服一組」。
**根因**：`GET /api/auth/me` 回傳 `teams: [{teamId, roleInTeam, isPrimary}]` — **不含團隊名稱**（實測確認，無 top-level teamName）。前端 `session.teamOptions()`（`auth/session.ts` readTeams）因而 fallback 成 `` `Team ${id}` ``。`quickAssignToMyTeam` 的 Toast 用 `myTeam.name`（來自 session）→「Team 1」。而 `AssignMenu` 下拉用 `/api/teams`（含名稱）→ 正確。屬 team-manager-access 期間就存在的 `/me` 契約缺口，被 Phase 2.1 的快捷 Toast 暴露。
**重現**：任一 agent 登入 → 開對話 → 點 composer「指給我的團隊」→ Toast 顯示「Team {id}」。
**建議修法**：`/me` 的 teams[] 補 `name`（backend join team_name），或前端 session 從 teamsStore 解析名稱。

### 🟡 F3 — 指派/轉移後客戶面板團隊標示未即時刷新（已知延後項）— ✅ 已修（`c2cc6cc`）
**嚴重度：低（重載即正確，後端已持久化）**

**現象**：thread header 轉移團隊成功（Toast + 後端 team_id 已更新），但右側客戶面板「指派團隊 → 團隊」仍顯示舊團隊，直到重載頁面才更新。
**根因**：`AssignMenu` 的 `onResult` 只發 Toast，未回寫 `meta.teamId/teamName`（Thread 的 `meta` 由開啟對話時的 `/api/conversations/:id` 載入，之後不隨指派刷新）。此為記憶中已記錄的延後項。
**重現**：開對話 → header 指派/轉移到另一團隊 → 看右側面板「團隊」仍為舊值。
**建議修法**：`onResult`（成功時）觸發 meta 重載或樂觀更新 `meta.teamId/teamName`。

---

## 未能於本次自動化驗證的項目（非缺陷，補充說明）

| 項目 | 原因 | 現有覆蓋 |
|------|------|----------|
| 樂觀更新 pending 半透明過渡 | 送出→確認太快，未截到過渡幀 | 功能正常；`Thread.test` 覆蓋 optimistic confirm/rollback |
| 圖片拖放 / 貼上 | CDP 難可靠模擬 drag/paste 檔案 | 與選檔共用 `addFiles`；選檔已驗證 |
| 新訊息音效 + 桌面通知 | headless 無音訊輸出 / OS Notification / 需分頁背景+授權 | `incomingAlerts.test`（4 tests） |
| 「連線已恢復」過渡 banner | 2.5s 過渡窗，未截到 | 斷線→消失行為已驗；`ConnectionBanner` 邏輯有測試 |

---

## 總結

**近三輪前端大改的核心 UX 功能，實走 11 個關鍵流程全部 PASS**，包含最容易在重構中壞掉的即時進線（webhook→置頂+badge）、樂觀送訊、inline 指派持久化、離線 banner、錯誤重試、深色模式（含審查點名的 MetricsView）、鍵盤可及性與 Modal focus trap。整體品質良好、可上線。

### 可以直接修（低風險、機械性）— 全數已完成
1. ~~**F1 — seed.rs 補 `updated_at`**（或 migration 0021 加 `DEFAULT now()`）。~~ ✅ `c5dea43`，採 seed 補欄（未動 schema）。
2. ~~**F2 — `/me` teams[] 補團隊名**（或前端 session 從 teamsStore 補名）。~~ ✅ `b97ad9c`（後端 join 團隊名）；`bc9932d` 一併讓登入回應帶 teams。
3. ~~**F3 — 指派後刷新 `meta.teamId/teamName`**（`onResult` 回寫）。~~ ✅ `c2cc6cc`。

### 需要討論 — 已定案
- 無阻斷性爭議項。~~唯一需拿捏的是 **F1 修法**~~ → 已採 seed 補欄（不動 schema）。
- ~~**F2/F3 是否納入本輪**~~ → 決定納入，兩項於報告產出同日修完並以截圖 15 驗證。

### 測試環境殘留（本次為驗證而造，非程式碼變更）
- runtime `ALTER conversations.updated_at SET DEFAULT now()`（F1 解阻）
- 造帳 `agent@example.com`、第二團隊「客服二組」、agent 加入兩團隊、一則測試通知、一則 webhook inbound 訊息
- 後端曾以 `LINE_CHANNEL_SECRET=qatestsecret` 啟動（供 webhook 簽章測試）

這些都在 `mcss-qa-db` 這個**臨時 docker 容器**內，`docker rm -f mcss-qa-db` 即全清；未觸及專案檔案或 git。
