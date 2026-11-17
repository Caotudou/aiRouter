if (window.__TAURI__) {
  const invoke = window.__TAURI__.core.invoke;
  window.router = Object.freeze({
    status: () => invoke('router_status'),
    refresh: () => invoke('router_refresh'),
    start: () => invoke('router_start'),
    stop: () => invoke('router_stop'),
    copy: (field) => invoke('router_copy', { field }),
    test: () => invoke('router_test'),
    chooseBinary: () => invoke('router_choose_binary'),
    save: (settings) => invoke('router_save', { settings }),
  });
}
