// 企微（智造 aibot）WS 适配器（外挂适配器包 v1）。
//
//! 协议与宿主内置实现（crates/bots-service/src/wecom_protocol.rs、wecom_payload.rs、
//! wecom_ws.rs）同源：wss://openws.work.weixin.qq.com，命令帧
//! aibot_subscribe / ping / aibot_respond_msg / aibot_send_msg /
//! aibot_upload_media_{init,chunk,finish}；服务端 aibot_msg_callback /
//! aibot_event_callback。回复走流式占位（stream）收口，长文本分片 markdown
//! 主动发送；媒体三步分片上传（512KiB/片，base64 + md5）。
//!
//! 零 npm 依赖：Node 22+ 原生 WebSocket 与 crypto。

import crypto from "node:crypto";

const WS_URL = "wss://openws.work.weixin.qq.com";
const CMD_SUBSCRIBE = "aibot_subscribe";
const CMD_HEARTBEAT = "ping";
const CMD_RESPONSE = "aibot_respond_msg";
const CMD_SEND_MSG = "aibot_send_msg";
const CMD_UPLOAD_INIT = "aibot_upload_media_init";
const CMD_UPLOAD_CHUNK = "aibot_upload_media_chunk";
const CMD_UPLOAD_FINISH = "aibot_upload_media_finish";
const CMD_CALLBACK = "aibot_msg_callback";
const CMD_EVENT_CALLBACK = "aibot_event_callback";
const HEARTBEAT_MS = 30_000;
const CHUNK_BYTES = 512 * 1024;
const MAX_CHUNKS = 100;
const IMAGE_LIMIT = 10 * 1024 * 1024;
const FILE_LIMIT = 50 * 1024 * 1024;
const STREAM_TEXT_LIMIT = 1800;
const ACK_TIMEOUT_MS = 10_000;

let config = null;
let ws = null;
let heartbeatTimer = null;
let aborted = false;
let authOk = false;
const streams = new Map(); // req_id → stream_id（流式占位）
const pendingAcks = new Map(); // req_id → {resolve, reject, timer}

function send(line) {
  process.stdout.write(JSON.stringify(line) + "\n");
}
const status = (s, detail = null) => send({ event: "status", status: s, detail });
const log = (line) => send({ event: "log", line });
const reqId = (prefix = "req") => `${prefix}_${Date.now()}_${Math.random().toString(16).slice(2, 10)}`;

function frame(cmd, req, body) {
  return { cmd, headers: { req_id: req }, body };
}

function awaitAck(req) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingAcks.delete(req);
      reject(new Error(`回执超时（${req}）`));
    }, ACK_TIMEOUT_MS);
    pendingAcks.set(req, { resolve, reject, timer });
  });
}

function rawSend(value) {
  if (!ws || ws.readyState !== 1) throw new Error("企微连接未就绪");
  ws.send(JSON.stringify(value));
}

async function request(cmd, req, body) {
  const waiter = awaitAck(req);
  rawSend(frame(cmd, req, body));
  return waiter;
}

function md5Hex(bytes) {
  return crypto.createHash("md5").update(bytes).digest("hex");
}

function classifyServerFrame(frame) {
  const cmd = frame.cmd || "";
  const req = frame.headers?.req_id || "";
  const errcode = frame.errcode ?? 0;
  const errmsg = frame.errmsg || "";
  if (cmd === CMD_CALLBACK) return { kind: "message", req, body: frame.body || {} };
  if (cmd === CMD_EVENT_CALLBACK) {
    const disconnected = frame.body?.event?.eventtype === "disconnected_event";
    return { kind: "event", req, body: frame.body || {}, disconnected };
  }
  if (req.startsWith(CMD_SUBSCRIBE)) {
    return errcode === 0 ? { kind: "auth_ok" } : { kind: "auth_failed", errcode, errmsg };
  }
  if (req.startsWith(CMD_HEARTBEAT)) return { kind: "heartbeat" };
  if (pendingAcks.has(req)) {
    return errcode === 0
      ? { kind: "ack_ok", req, body: frame.body || {} }
      : { kind: "ack_error", req, errcode, errmsg };
  }
  return { kind: "unknown" };
}

async function connectLoop() {
  while (!aborted) {
    status("connecting");
    authOk = false;
    try {
      ws = new WebSocket(WS_URL);
      await openOnce(ws);
      const subscribeReq = reqId(CMD_SUBSCRIBE);
      rawSend(frame(CMD_SUBSCRIBE, subscribeReq, { bot_id: config.botId, secret: config.secret }));
      const auth = await waitAuth();
      if (!auth) throw new Error("订阅认证超时");
      authOk = true;
      status("connected");
      log("企微 WS 已认证");
      await pump(ws);
    } catch (error) {
      const message = String(error.message || error);
      status(/认证|secret/i.test(message) ? "error" : "reconnecting", message);
    }
    stopHeartbeat();
    try { ws?.close(); } catch {}
    ws = null;
    streams.clear();
    if (!aborted) await sleep(3_000);
  }
}

function waitAuth() {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), ACK_TIMEOUT_MS);
    const check = () => {
      if (authOk) {
        clearTimeout(timer);
        resolve(true);
      } else {
        setTimeout(check, 200);
      }
    };
    check();
  });
}

function openOnce(socket) {
  return new Promise((resolve, reject) => {
    if (socket.readyState === 1) return resolve();
    socket.addEventListener("open", () => resolve(), { once: true });
    socket.addEventListener("error", (e) => reject(new Error(`WS 连接失败：${e.message}`)), { once: true });
  });
}

function startHeartbeat() {
  heartbeatTimer = setInterval(() => {
    if (ws?.readyState === 1) rawSend(frame(CMD_HEARTBEAT, reqId(CMD_HEARTBEAT)));
  }, HEARTBEAT_MS);
}

function stopHeartbeat() {
  if (heartbeatTimer) clearInterval(heartbeatTimer);
  heartbeatTimer = null;
}

async function pump(socket) {
  startHeartbeat();
  await new Promise((resolve) => {
    socket.addEventListener("message", (raw) => {
      let frame;
      try { frame = JSON.parse(typeof raw === "string" ? raw : raw.data); } catch { return; }
      const classified = classifyServerFrame(frame);
      switch (classified.kind) {
        case "auth_ok":
          authOk = true;
          break;
        case "auth_failed":
          log(`认证失败 ${classified.errcode}: ${classified.errmsg}`);
          status("error", `认证失败 ${classified.errcode}: ${classified.errmsg}`);
          break;
        case "ack_ok":
        case "ack_error": {
          const waiter = pendingAcks.get(classified.req);
          if (waiter) {
            clearTimeout(waiter.timer);
            pendingAcks.delete(classified.req);
            if (classified.kind === "ack_ok") waiter.resolve(classified.body);
            else waiter.reject(new Error(`企微命令失败 ${classified.errcode}: ${classified.errmsg}`));
          }
          break;
        }
        case "heartbeat":
          break;
        case "event":
          if (classified.disconnected) {
            log("被新连接踢下线");
            try { socket.close(); } catch {}
            resolve();
          }
          break;
        case "message": {
          const body = classified.body;
          const text = messageText(body);
          const messageId = body.msgid || body.msg_id || "";
          if (!text && !hasAttachment(body)) break;
          const streamId = reqId("stream");
          if (classified.req) streams.set(classified.req, streamId);
          send({
            event: "message",
            messageId,
            senderId: body.from?.userid || "",
            senderNick: body.from?.name || null,
            chatType: body.chattype || (body.chatid ? "group" : "private"),
            chatId: body.chatid || "",
            text,
            contextToken: JSON.stringify({ reqId: classified.req, chatId: body.chatid || body.from?.userid || "" }),
          });
          break;
        }
        default:
          break;
      }
    });
    socket.addEventListener("close", () => resolve());
    socket.addEventListener("error", () => resolve());
  });
}

function hasAttachment(body) {
  if (body.msgtype === "mixed") return (body.mixed?.msg_item || []).some((item) => item.msgtype !== "text");
  return ["image", "file", "voice", "video"].includes(body.msgtype || "");
}

function messageText(body) {
  const msgtype = body.msgtype || "";
  if (msgtype === "text") return (body.text?.content || "").trim();
  if (msgtype === "mixed") {
    const parts = (body.mixed?.msg_item || [])
      .filter((item) => item.msgtype === "text")
      .map((item) => item.text?.content || "");
    return parts.join("").trim();
  }
  return "";
}

function mediaKind(path) {
  const ext = (path.split(".").pop() || "").toLowerCase();
  return ["png", "jpg", "jpeg", "gif", "bmp", "webp"].includes(ext) ? "image" : "file";
}

async function uploadMedia(path, kind) {
  const bytes = await import("node:fs/promises").then((fs) => fs.readFile(path));
  const total = bytes.length;
  const limit = kind === "image" ? IMAGE_LIMIT : FILE_LIMIT;
  if (total > limit) throw new Error(`文件超过企微上传上限（${Math.round(limit / 1024 / 1024)}MB）`);
  if (total === 0) throw new Error("文件为空，无法发送。");
  const totalChunks = Math.ceil(total / CHUNK_BYTES);
  if (totalChunks > MAX_CHUNKS) throw new Error(`文件超过企微上传上限（${totalChunks} 个分片，最多 ${MAX_CHUNKS} 个）。`);
  const md5 = md5Hex(bytes);
  const initRes = await request(CMD_UPLOAD_INIT, reqId(CMD_UPLOAD_INIT), {
    type: kind,
    filename: path.split(/[\\/]/).pop(),
    total_size: total,
    total_chunks: totalChunks,
    md5,
  });
  const uploadId = initRes.upload_id || "";
  if (!uploadId) throw new Error("企微上传初始化失败：缺少 upload_id。");
  for (let index = 0; index < totalChunks; index += 1) {
    const start = index * CHUNK_BYTES;
    const end = Math.min(start + CHUNK_BYTES, total);
    await request(CMD_UPLOAD_CHUNK, reqId(CMD_UPLOAD_CHUNK), {
      upload_id: uploadId,
      chunk_index: index,
      base64_data: bytes.subarray(start, end).toString("base64"),
    });
  }
  const finishRes = await request(CMD_UPLOAD_FINISH, reqId(CMD_UPLOAD_FINISH), { upload_id: uploadId });
  const mediaId = finishRes.media_id || "";
  if (!mediaId) throw new Error("企微上传完成失败：缺少 media_id。");
  return mediaId;
}

async function reply(op) {
  const ctx = safeParse(op.context_token);
  const reqIdValue = ctx.reqId || "";
  const chatId = op.chat_id || ctx.chatId || "";
  const kind = op.media_kind || null;
  let mediaId = null;
  if (op.media_path) {
    mediaId = await uploadMedia(op.media_path, kind || mediaKind(op.media_path));
  }
  if (mediaId) {
    const body = reqIdValue
      ? { msgtype: kind, [kind]: { media_id: mediaId } }
      : { chatid: chatId, msgtype: kind, [kind]: { media_id: mediaId } };
    if (reqIdValue) await request(CMD_RESPONSE, reqIdValue, body);
    else await request(CMD_SEND_MSG, reqId(CMD_SEND_MSG), body);
    if (op.caption) await sendMarkdown(chatId, op.caption, ctx.reqId && false);
    return;
  }
  const text = (op.text || "").trim();
  if (!text) return;
  if (reqIdValue) {
    const streamId = streams.get(reqIdValue) || reqId("stream");
    streams.delete(reqIdValue);
    const chunks = splitText(text, STREAM_TEXT_LIMIT);
    await request(CMD_RESPONSE, reqIdValue, {
      msgtype: "stream",
      stream: { id: streamId, finish: true, content: chunks[0] || "" },
    });
    for (const chunk of chunks.slice(1)) {
      await request(CMD_SEND_MSG, reqId(CMD_SEND_MSG), { chatid: chatId, msgtype: "markdown", markdown: { content: chunk } });
    }
    return;
  }
  await sendMarkdown(chatId, text, true);
}

async function sendMarkdown(chatId, text, split = true) {
  const chunks = split ? splitText(text, STREAM_TEXT_LIMIT) : [text];
  for (const chunk of chunks) {
    await request(CMD_SEND_MSG, reqId(CMD_SEND_MSG), { chatid: chatId, msgtype: "markdown", markdown: { content: chunk } });
  }
}

function splitText(text, limit) {
  if (text.length <= limit) return [text];
  const chunks = [];
  for (let index = 0; index < text.length; index += limit) {
    chunks.push(text.slice(index, index + limit));
  }
  return chunks;
}

function safeParse(text) {
  try { return JSON.parse(text || "{}") || {}; } catch { return {}; }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

process.stdin.setEncoding("utf8");
let buffer = "";
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index;
  while ((index = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (!line) continue;
    let op;
    try { op = JSON.parse(line); } catch { continue; }
    if (op.op === "start") {
      config = op.config;
      if (!config?.botId || !config?.secret) {
        status("error", "企微 Bot ID / Secret 未配置");
        continue;
      }
      aborted = false;
      streams.clear();
      connectLoop();
    } else if (op.op === "reply") {
      if (!authOk) {
        log("回复失败：企微连接未就绪");
        continue;
      }
      reply(op).catch((error) => log(`回复失败：${error.message || error}`));
    }
  }
});
process.stdin.on("end", () => {
  aborted = true;
  stopHeartbeat();
  try { ws?.close(); } catch {}
  process.exit(0);
});
send({ event: "ready" });
