"use strict";

const { listenerNew, listenerStart, listenerStop } = require("./index.node");

class WindowForegroundListener {
  constructor() {
    this.listener = listenerNew();
  }

  start(pid, cb) {
    listenerStart.call(this.listener, pid, (name) => {
      cb(name);
      return "";
    });
  }

  stop() {
    listenerStop.call(this.listener);
  }
}

module.exports = WindowForegroundListener;
