import WebSocket from "ws";

var DaemonConnection = class {
  ws = null;
  options;
  reconnectTimer = null;
  reconnectDelay = 1e3;
  maxReconnectDelay = 3e4;
  shouldConnect = true;
  constructor(options) {
    this.options = options;
  }
  connect() {
    this.shouldConnect = true;
    this.doConnect();
  }
  disconnect() {
    this.shouldConnect = false;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }
  send(msg) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }
  get connected() {
    return this.ws?.readyState === WebSocket.OPEN;
  }
  doConnect() {
    if (!this.shouldConnect) return;
    const wsUrl = this.options.serverUrl.replace(/^http/, "ws") + `/daemon/connect?key=${this.options.apiKey}`;
    console.log(`[Daemon] Connecting to ${this.options.serverUrl}...`);
    this.ws = new WebSocket(wsUrl);
    this.ws.on("open", () => {
      console.log("[Daemon] Connected to server");
      this.reconnectDelay = 1e3;
      this.options.onConnect();
    });
    this.ws.on("message", (data) => {
      try {
        const msg = JSON.parse(data.toString());
        this.options.onMessage(msg);
      } catch (err) {
        console.error("[Daemon] Invalid message from server:", err);
      }
    });
    this.ws.on("close", () => {
      console.log("[Daemon] Disconnected from server");
      this.options.onDisconnect();
      this.scheduleReconnect();
    });
    this.ws.on("error", (err) => {
      console.error("[Daemon] WebSocket error:", err.message);
    });
  }
  scheduleReconnect() {
    if (!this.shouldConnect) return;
    if (this.reconnectTimer) return;
    console.log(`[Daemon] Reconnecting in ${this.reconnectDelay}ms...`);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.doConnect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
  }
};

export { DaemonConnection };
