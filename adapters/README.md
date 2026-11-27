# IM 通道适配器包

外挂适配器：通道协议实现不编译进应用二进制，而是放数据目录
`{data_root}/v2/im-adapters/`，由应用以 Node 子进程 + stdio JSONL 桥接。
协议变了 → 发一个新的适配器包（在线更新）即可，应用不用重新安装。

## 包结构

```
manifest.json           {"version": N, "adapters": {<channel>: {"entry": "...", "providers": [...]}}}
dingtalk/adapter.mjs    钉钉 Stream（gateway open + REGISTER/PING/EVENT-ACK + sessionWebhook 回复）
feishu/adapter.mjs      飞书长连接（outbound_ws endpoint + 二进制帧 SIGNAL/DATA/CONTROL）
wecom/adapter.mjs       企微智造 aibot WS（subscribe/心跳/respond 流式/send/三步媒体上传）
```

零 npm 依赖：Node 22+ 原生 WebSocket / fetch / crypto。
Node 运行时由应用按需下载到 `{data_root}/runtime/node`（无系统安装）。

## stdio JSONL 协议

stdin（应用 → 适配器）：

```json
{"op":"start","bot_id":"...","config":{...通道凭据，由应用从 TokenVault 取...}}
{"op":"reply","context_token":"...","text":"...","media_path":null,"caption":null,"chat_id":null}
```

stdout（适配器 → 应用）：

```json
{"event":"ready"}
{"event":"status","status":"connecting|connected|reconnecting|error","detail":null}
{"event":"log","line":"..."}
{"event":"message","messageId":"...","senderId":"...","senderNick":null,
 "chatType":"private|group","chatId":"...","text":"...","contextToken":"..."}
```

`context_token` 是适配器自己的回复凭据（钉钉=sessionWebhook、飞书=JSON(chatId)、
企微=原消息 req_id），应用原样透传回 `op.reply`。

## 更新流程（应用侧）

1. `GET <源目录>/manifest.json`（源 URL 可配置，默认应用仓库 raw 地址）
2. 远端 `version` > 本地 `version` → `GET <package>`（manifest 内 `package` + `sha256`）
3. sha256 校验 → `tar -xf` 解压到临时目录 → 校验新 manifest → 原子 swap 数据目录
4. 重启所有适配器进程；失败自动回滚旧目录

## 构建发布

```
zip im-adapters.zip manifest.json dingtalk feishu wecom
sha256sum im-adapters.zip   # 写进远端 manifest 的 "sha256"
```
