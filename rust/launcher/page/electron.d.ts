// The two names Reforged's preload.ts imports from Electron, so that `LauncherApi` (its `typeof api`) can
// be type-checked here without installing Electron (tsconfig.json maps "electron" to this file).

export declare const contextBridge: { exposeInMainWorld(key: string, api: unknown): void };
export declare const ipcRenderer: {
  invoke(channel: string, ...args: unknown[]): Promise<any>;
  on(channel: string, listener: (event: unknown, ...args: any[]) => void): void;
};
