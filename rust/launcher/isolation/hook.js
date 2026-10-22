// The isolation hook (MIGRATION_PLAN.md L9, D4). Tauri runs it in a sandboxed frame of its own, with its
// own Content-Security-Policy and no network, and passes it every call the page makes before the call is
// encrypted for the backend. It lets through only the commands in commands.js (generated from the
// backend's list) and the few Tauri calls the page needs, with arguments of the right shape and size.
// Anything else is dropped: the call never settles, so the page's api.ts times it out.
// Keep this file free of dependencies and of anything but the list: the backend checks again anyway.
"use strict";

(function () {
  var COMMANDS = window.__DR_COMMANDS__;
  var SETTINGS_KEYS = window.__DR_SETTINGS_KEYS__;
  var EVENT_MAX = 64;

  // Tauri's own calls the page makes: events (listen/unlisten), the window controls and the title bar's
  // drag region (which calls without a window label)
  var WINDOW = { label: { kind: "optionalText", max: EVENT_MAX } };
  var TAURI = {
    "plugin:event|listen": { event: { kind: "text", max: EVENT_MAX }, target: { kind: "any" }, handler: { kind: "number" } },
    "plugin:event|unlisten": { event: { kind: "text", max: EVENT_MAX }, eventId: { kind: "number" } },
    "plugin:window|minimize": WINDOW,
    "plugin:window|toggle_maximize": WINDOW,
    "plugin:window|is_maximized": WINDOW,
    "plugin:window|start_dragging": WINDOW,
    "plugin:window|internal_toggle_maximize": WINDOW,
    "plugin:window|close": WINDOW,
  };

  var own = function (object, key) {
    return Object.prototype.hasOwnProperty.call(object, key);
  };

  function settingsPatch(value) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
    var keys = Object.keys(value);
    for (var i = 0; i < keys.length; i++) {
      var field = value[keys[i]];
      if (SETTINGS_KEYS.indexOf(keys[i]) < 0) return false;
      if (typeof field === "string" ? field.length > 16 : typeof field !== "number" && typeof field !== "boolean") return false;
    }
    return true;
  }

  function fits(spec, value, present) {
    switch (spec.kind) {
      case "text":
        return present && typeof value === "string" && value.length <= spec.max;
      case "optionalText":
        return !present || value === undefined || value === null || (typeof value === "string" && value.length <= spec.max);
      case "flag":
        return present && typeof value === "boolean";
      case "number":
        return present && typeof value === "number" && isFinite(value);
      case "settingsPatch":
        return present && settingsPatch(value);
      case "channel":
        return present && typeof value === "string" && /^__CHANNEL__:[0-9]{1,16}$/.test(value);
      case "any":
        return true;
      default:
        return false;
    }
  }

  function allowed(cmd, args) {
    if (typeof cmd !== "string") return false;
    var specs = own(COMMANDS, cmd) ? COMMANDS[cmd] : own(TAURI, cmd) ? TAURI[cmd] : null;
    if (specs === null) return false;
    if (args === undefined || args === null) args = {};
    if (typeof args !== "object" || Array.isArray(args)) return false;
    var names = Object.keys(args);
    for (var i = 0; i < names.length; i++) {
      if (!own(specs, names[i])) return false;
    }
    var expected = Object.keys(specs);
    for (var j = 0; j < expected.length; j++) {
      if (!fits(specs[expected[j]], args[expected[j]], own(args, expected[j]))) return false;
    }
    return true;
  }

  var never = new Promise(function () {});

  window.__TAURI_ISOLATION_HOOK__ = function (payload) {
    return payload !== null && typeof payload === "object" && allowed(payload.cmd, payload.payload) ? payload : never;
  };
})();
