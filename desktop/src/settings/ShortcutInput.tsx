import React, { useState } from "react";

interface ShortcutInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
}

export function ShortcutInput({ value, onChange, placeholder = "未设置" }: ShortcutInputProps) {
  const [recording, setRecording] = useState(false);

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    e.preventDefault();
    e.stopPropagation();

    // 仅在未按修饰键时按 Backspace / Delete 清空
    const hasModifier = e.ctrlKey || e.altKey || e.shiftKey || e.metaKey;
    if ((e.key === "Backspace" || e.key === "Delete") && !hasModifier) {
      onChange("");
      setRecording(false);
      return;
    }

    if (e.key === "Escape") {
      setRecording(false);
      return;
    }

    // 仅按下修饰键时不结算
    if (["Control", "Alt", "Shift", "Meta", "Command"].includes(e.key)) {
      return;
    }

    const isMac = typeof navigator !== "undefined" && /Mac|iPod|iPhone|iPad/.test(navigator.platform);
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    if (e.metaKey) parts.push(isMac ? "Command" : "Super");

    let key = e.key;
    if (key === " ") key = "Space";
    else if (key.length === 1) key = key.toUpperCase();
    else if (key.startsWith("Arrow")) {
      // Keep ArrowUp, ArrowDown, ArrowLeft, ArrowRight in PascalCase
    } else if (/^F\d{1,2}$/i.test(key)) {
      key = key.toUpperCase();
    }

    // 全局快捷键必须包含至少一个修饰键，或者为 F1~F24，避免单独按单字符拦截全局按键
    const isFunctionKey = /^F\d{1,2}$/i.test(key);
    if (parts.length === 0 && !isFunctionKey) {
      return;
    }

    parts.push(key);
    onChange(parts.join("+"));
    setRecording(false);
  }

  const parts = value ? value.split("+").filter(Boolean) : [];

  return (
    <div className="shortcut-input-wrap">
      <input
        type="text"
        className={`input shortcut-input ${recording ? "recording" : ""}`}
        value={recording ? "按下新快捷键组合…" : value}
        placeholder={placeholder}
        readOnly
        onFocus={() => setRecording(true)}
        onBlur={() => setRecording(false)}
        onKeyDown={handleKeyDown}
      />
      {!recording && parts.length > 0 && (
        <div className="shortcut-keys">
          {parts.map((p, i) => (
            <kbd key={i} className="kbd-badge">
              {p}
            </kbd>
          ))}
        </div>
      )}
      {value && (
        <button
          type="button"
          className="shortcut-clear-btn"
          onClick={(e) => {
            e.stopPropagation();
            onChange("");
          }}
          title="清除快捷键"
        >
          ✕
        </button>
      )}
    </div>
  );
}
