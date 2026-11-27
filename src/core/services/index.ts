export type { AppDirs, BinaryFile, DirEntry, FsService, Stat, TextFile } from './fs';
export type { ProcHandle, ProcInfo, ProcOptions, ProcService } from './proc';
export type { PtyInfo, PtyOptions, PtyProcess, PtyService } from './pty';
export type { PortInfo, SerialHandle, SerialOptions, SerialService } from './serial';
export type {
  FsChange,
  FsToolsService,
  SearchMatch,
  SearchOptions,
  SearchResult,
  WatchInfo,
  WatchOptions,
} from './fs-tools';

export { TauriFsService } from './fs';
export { TauriProcService } from './proc';
export { TauriPtyService } from './pty';
export { TauriSerialService } from './serial';
export { TauriFsToolsService } from './fs-tools';
export { ServiceRegistry } from './registry';
