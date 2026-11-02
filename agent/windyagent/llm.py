"""Talks to the language model. Two wire formats cover almost everything:

  anthropic  ->  POST {base}/v1/messages           (Claude)
  openai     ->  POST {base}/chat/completions      (OpenAI, Gemini, OpenRouter, LM Studio, ...)
  ollama     ->  POST {base}/api/chat              (Ollama's own API: bigger context window, model kept loaded)

Inside the agent, messages are always kept in the Anthropic block format:
    {"role": "user"|"assistant", "content": str | [blocks]}
    blocks: {"type": "text", "text"} | {"type": "tool_use", "id", "name", "input"}
            | {"type": "tool_result", "tool_use_id", "content": str}
Only urllib is used, so there is nothing extra to install.
"""
from __future__ import annotations

import json
import time
import urllib.error
import urllib.request
import uuid

from . import config


class LLMError(Exception):
    pass


class Cancelled(Exception):
    pass


def _http_json(url: str, headers: dict, body: dict, timeout: int = 180, local: bool = False) -> dict:
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json", **headers}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            j = json.loads(raw)
            msg = (j.get("error") or {}).get("message") if isinstance(j.get("error"), dict) else j.get("error") or j.get("message")
        except Exception:
            msg = None
        if local and e.code == 404:
            msg = f"{msg or 'model not found'}. Run  ollama pull <model>  or pick a name from  ollama list  in Tobi's settings."
        err = LLMError(f"HTTP {e.code}: {msg or raw[:300]}")
        err.status = e.code  # type: ignore[attr-defined]
        raise err from None
    except urllib.error.URLError as e:
        if local:
            raise LLMError("Ollama is not running. Open the Ollama app (llama icon in the tray) and try again.") from None
        raise LLMError(f"Could not reach {url.split('/')[2]}: {e.reason}") from None
    except TimeoutError:
        raise LLMError("The model took too long to answer.") from None


class LLM:
    def __init__(self, cfg: dict):
        self.r = config.resolved(cfg)
        self.num_ctx = int(cfg.get("num_ctx") or 8192)
        self.label = str((config.PROVIDERS.get(self.r.get("provider")) or {}).get("label") or self.r.get("provider") or "")

    @property
    def name(self) -> str:
        """Which AI this really is, e.g. "Ollama (local, no key) - qwen2.5:7b" (shown in the humanizer's log)."""
        return f"{self.label.split(' (')[0]} {self.r.get('model', '')}".strip()

    # ------------------------------------------------------------------ public
    def complete(self, system: str, messages: list[dict], tools: list[dict] | None = None, max_tokens: int = 4096,
                 cancel=None) -> dict:
        """Returns {"blocks": [...anthropic blocks...], "stop": "tool_use" | "end"}."""
        last: Exception | None = None
        for attempt in range(4):
            if cancel and cancel.is_set():
                raise Cancelled()
            try:
                if self.r["style"] == "anthropic":
                    return self._anthropic(system, messages, tools, max_tokens)
                if self.r["style"] == "ollama":
                    return self._ollama(system, messages, tools, max_tokens)
                return self._openai(system, messages, tools, max_tokens)
            except LLMError as e:
                last = e
                status = getattr(e, "status", 0)
                if status in (429, 500, 502, 503, 504, 529) and attempt < 3:
                    time.sleep(2 * (attempt + 1))
                    continue
                raise
        raise last or LLMError("unknown error")

    def ask(self, system: str, user: str, max_tokens: int = 500) -> str:
        """One question, one text answer (used by the humanizer's sentence judge)."""
        out = self.complete(system, [{"role": "user", "content": user}], None, max_tokens)
        return "".join(b.get("text", "") for b in out["blocks"] if b.get("type") == "text")

    # ------------------------------------------------------------------ anthropic
    def _anthropic(self, system, messages, tools, max_tokens):
        base = self.r["base_url"]
        url = base + ("/messages" if base.endswith("/v1") else "/v1/messages")
        body: dict = {"model": self.r["model"], "max_tokens": max_tokens, "system": system, "messages": messages}
        if tools:
            body["tools"] = [{"name": t["name"], "description": t["description"], "input_schema": t["schema"]} for t in tools]
        j = _http_json(url, {"x-api-key": self.r["api_key"], "anthropic-version": "2023-06-01"}, body)
        blocks = []
        for b in j.get("content", []):
            if b.get("type") == "text" and b.get("text"):
                blocks.append({"type": "text", "text": b["text"]})
            elif b.get("type") == "tool_use":
                blocks.append({"type": "tool_use", "id": b["id"], "name": b["name"], "input": b.get("input") or {}})
        stop = "tool_use" if any(b["type"] == "tool_use" for b in blocks) else "end"
        return {"blocks": blocks, "stop": stop}

    # ------------------------------------------------------------------ openai-compatible
    @staticmethod
    def to_openai_messages(system: str, messages: list[dict]) -> list[dict]:
        out: list[dict] = [{"role": "system", "content": system}]
        for m in messages:
            c = m["content"]
            if isinstance(c, str):
                out.append({"role": m["role"], "content": c})
                continue
            if m["role"] == "assistant":
                text = "".join(b.get("text", "") for b in c if b["type"] == "text")
                calls = [
                    {"id": b["id"], "type": "function", "function": {"name": b["name"], "arguments": json.dumps(b.get("input") or {})}}
                    for b in c if b["type"] == "tool_use"
                ]
                msg: dict = {"role": "assistant", "content": text or None}
                if calls:
                    msg["tool_calls"] = calls
                out.append(msg)
            else:
                text = "".join(b.get("text", "") for b in c if b["type"] == "text")
                for b in c:
                    if b["type"] == "tool_result":
                        out.append({"role": "tool", "tool_call_id": b["tool_use_id"], "content": str(b.get("content", ""))})
                if text:
                    out.append({"role": "user", "content": text})
        return out

    def _openai(self, system, messages, tools, max_tokens):
        url = self.r["base_url"] + "/chat/completions"
        body: dict = {"model": self.r["model"], "messages": self.to_openai_messages(system, messages)}
        if tools:
            body["tools"] = [
                {"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["schema"]}}
                for t in tools
            ]
        headers = {"Authorization": f"Bearer {self.r['api_key']}"} if self.r["api_key"] else {}
        j = _http_json(url, headers, body)
        try:
            msg = j["choices"][0]["message"]
        except (KeyError, IndexError):
            raise LLMError(f"Unexpected reply from the model: {str(j)[:200]}") from None
        blocks = []
        if msg.get("content"):
            blocks.append({"type": "text", "text": msg["content"]})
        for tc in msg.get("tool_calls") or []:
            fn = tc.get("function", {})
            try:
                args = json.loads(fn.get("arguments") or "{}")
            except json.JSONDecodeError:
                args = {}
            blocks.append({"type": "tool_use", "id": tc.get("id") or "call_" + uuid.uuid4().hex[:12],
                           "name": fn.get("name", ""), "input": args if isinstance(args, dict) else {}})
        stop = "tool_use" if any(b["type"] == "tool_use" for b in blocks) else "end"
        return {"blocks": blocks, "stop": stop}

    # ------------------------------------------------------------------ ollama (native API)
    def _ollama_messages(self, system: str, messages: list[dict]) -> list[dict]:
        out: list[dict] = [{"role": "system", "content": system}]
        for m in messages:
            c = m["content"]
            if isinstance(c, str):
                out.append({"role": m["role"], "content": c})
            elif m["role"] == "assistant":
                text = "".join(b.get("text", "") for b in c if b["type"] == "text")
                msg: dict = {"role": "assistant", "content": text}
                calls = [{"function": {"name": b["name"], "arguments": b.get("input") or {}}} for b in c if b["type"] == "tool_use"]
                if calls:
                    msg["tool_calls"] = calls
                out.append(msg)
            else:
                names = {}
                for prev in messages:
                    if prev["role"] == "assistant" and isinstance(prev["content"], list):
                        for b in prev["content"]:
                            if b["type"] == "tool_use":
                                names[b["id"]] = b["name"]
                for b in c:
                    if b["type"] == "tool_result":
                        out.append({"role": "tool", "tool_name": names.get(b["tool_use_id"], ""), "content": str(b.get("content", ""))})
                text = "".join(b.get("text", "") for b in c if b["type"] == "text")
                if text:
                    out.append({"role": "user", "content": text})
        return out

    def _ollama(self, system, messages, tools, max_tokens):
        body: dict = {
            "model": self.r["model"], "stream": False, "keep_alive": "30m",
            "messages": self._ollama_messages(system, messages),
            "options": {"num_ctx": self.num_ctx, "temperature": 0.2, "num_predict": max_tokens},
        }
        if tools:
            body["tools"] = [{"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["schema"]}}
                             for t in tools]
        j = _http_json(self.r["base_url"] + "/api/chat", {}, body, timeout=600, local=True)
        msg = j.get("message") or {}
        blocks = []
        if msg.get("content"):
            blocks.append({"type": "text", "text": msg["content"]})
        for tc in msg.get("tool_calls") or []:
            fn = tc.get("function", {})
            args = fn.get("arguments") or {}
            if isinstance(args, str):
                try:
                    args = json.loads(args)
                except json.JSONDecodeError:
                    args = {}
            blocks.append({"type": "tool_use", "id": "call_" + uuid.uuid4().hex[:12], "name": fn.get("name", ""),
                           "input": args if isinstance(args, dict) else {}})
        stop = "tool_use" if any(b["type"] == "tool_use" for b in blocks) else "end"
        return {"blocks": blocks, "stop": stop}

    def warm_up(self) -> None:
        """Load the local model into memory now, so the first real question is not slow."""
        if self.r["style"] != "ollama":
            return
        try:
            _http_json(self.r["base_url"] + "/api/generate", {}, {"model": self.r["model"], "keep_alive": "30m"}, timeout=300, local=True)
        except Exception:
            pass
