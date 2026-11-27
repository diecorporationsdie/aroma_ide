import { EventBus } from './core/event-bus';
import { CommandRegistry } from './core/command-registry';
import { Storage, MemoryStorageBackend, TauriStorageBackend } from './core/storage';
import {
  ServiceRegistry,
  TauriFsService,
  TauriFsToolsService,
  TauriProcService,
  TauriPtyService,
  TauriSerialService,
} from './core/services';
import { LayoutManager, DomUiHost, SplashScreen } from './core/ui';
import {
  BlobModuleLoader,
  CapabilityRegistry,
  FilesystemPluginSource,
  IORegistry,
  PluginHost,
} from './core/plugin';
import { listen } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';

const MIN_SPLASH_MS = 700;

export interface CoreServices {
  readonly events: EventBus;
  readonly commands: CommandRegistry;
  readonly storage: Storage;
  readonly services: ServiceRegistry;
  dispose(): void;
}

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI__' in window;
}

export function createCore(): CoreServices {
  const events = new EventBus();
  const commands = new CommandRegistry();
  const storage = new Storage(
    isTauri() ? new TauriStorageBackend() : new MemoryStorageBackend(),
  );
  const services = new ServiceRegistry();

  // Имена сервисов — публичный контракт плагинов. `core.fs` и `pty` уже
  // использовались, остальные добавлены для новых возможностей.
  services.register('core.fs', new TauriFsService());
  services.register('pty', new TauriPtyService());
  services.register('proc', new TauriProcService());
  services.register('serial', new TauriSerialService());
  services.register('fs.tools', new TauriFsToolsService());
  services.register('vendor.xterm', { Terminal, FitAddon });

  return {
    events,
    commands,
    storage,
    services,
    dispose() {
      services.dispose();
      events.dispose();
      commands.dispose();
    },
  };
}

function renderFallback(layout: LayoutManager, title: string, hint: string): void {
  const wrap = document.createElement('div');
  wrap.style.cssText =
    'display:flex;flex-direction:column;align-items:center;justify-content:center;' +
    'height:100%;gap:8px;padding:32px;text-align:center;color:var(--ide-fg-muted);';
  const h = document.createElement('div');
  h.textContent = title;
  h.style.cssText = 'font-size:14px;color:var(--ide-fg);';
  const p = document.createElement('div');
  p.textContent = hint;
  p.style.fontSize = '12px';
  wrap.append(h, p);
  layout.region('main').appendChild(wrap);
}

async function bootstrap(root: HTMLElement): Promise<void> {
  const splash = new SplashScreen('AROMA IDE');
  const startedAt = performance.now();

  const core = createCore();
  core.commands.register('core.ping', () => 'pong', 'Healthcheck');

  const layout = new LayoutManager(core.storage, 'AROMA IDE');
  await layout.mount(root);
  root.appendChild(splash.element);

  const io = new IORegistry();
  const capabilities = new CapabilityRegistry();
  const source = new FilesystemPluginSource();

  const host = new PluginHost({
    source,
    loader: new BlobModuleLoader(),
    contextDeps: {
      commands: core.commands,
      events: core.events,
      storage: core.storage,
      services: core.services,
      ui: new DomUiHost(layout),
      io,
      capabilities,
    },
  });

  // --- Горячая перезагрузка плагинов -------------------------------------
  // Бэкенд следит за каталогами плагинов и присылает id изменившихся файлов.
  // Без этого правка index.js требовала бы перезапуска IDE — а это основной
  // цикл разработки в плагинной IDE.
  let unlistenPlugins: (() => void) | null = null;
  if (isTauri()) {
    try {
      unlistenPlugins = await listen<{ ids: string[] }>('plugins:changed', (event) => {
        const ids = event.payload?.ids ?? [];
        if (ids.length === 0) return;
        void host.reloadMany(ids).catch((e) => {
          console.error('[plugins] hot reload failed', e);
        });
      });
    } catch (e) {
      console.warn('[plugins] hot reload unavailable', e);
    }
  }

  try {
    splash.setStatus('Поиск плагинов…');
    const manifests = await host.listAvailable();
    splash.setPlugins(manifests.map((m) => ({ id: m.id, name: m.name })));

    const total = manifests.length;
    let completed = 0;
    splash.setStatus(total === 0 ? 'Плагины не найдены' : `Загрузка плагинов (0/${total})…`);

    await host.activateAll((event) => {
      const name = event.manifest.name || event.manifest.id;
      switch (event.type) {
        case 'start':
          splash.setChip(event.manifest.id, 'loading');
          splash.setStatus(`Загрузка: ${name}…`);
          break;
        case 'ok':
          splash.setChip(event.manifest.id, 'ok');
          completed++;
          splash.setStatus(`Загрузка плагинов (${completed}/${total})…`);
          break;
        case 'error':
          splash.setChip(event.manifest.id, 'error');
          completed++;
          splash.setStatus(`Загрузка плагинов (${completed}/${total})…`);
          break;
        case 'skipped':
          splash.setChip(event.manifest.id, 'skipped');
          completed++;
          splash.setStatus(`Загрузка плагинов (${completed}/${total})…`);
          break;
      }
    });

    const active = host.listActive();
    if (active.length === 0) {
      // Причина важнее факта: «плагинов нет» и «плагины битые» — разные
      // проблемы, и пользователь должен видеть, какая именно.
      const diagnostics = await source.diagnostics().catch(() => []);
      splash.setStatus('Плагины не найдены');
      const hint =
        diagnostics.length > 0
          ? `Плагины найдены, но не загружены:\n${diagnostics
              .map((d) => `${d.id}: ${d.error}`)
              .join('\n')}`
          : 'Проверьте папку resources/plugins/ (или plugins/).';
      renderFallback(layout, 'Плагины не найдены', hint);
      if (diagnostics.length > 0) {
        console.warn('[plugins] diagnostics', diagnostics);
      }
    }

    const elapsed = performance.now() - startedAt;
    if (elapsed < MIN_SPLASH_MS) {
      await new Promise<void>((r) => setTimeout(r, MIN_SPLASH_MS - elapsed));
    }
    await splash.fadeOut();
  } catch (e) {
    splash.setStatus(`Ошибка запуска: ${String(e)}`);
    console.error('[bootstrap] failed', e);
    renderFallback(layout, 'Не удалось запустить IDE', String(e));
    await new Promise<void>((r) => setTimeout(r, 1200));
  } finally {
    splash.dispose();
  }

  window.addEventListener('beforeunload', () => {
    unlistenPlugins?.();
    void host.shutdown();
    host.dispose();
    capabilities.dispose();
    io.dispose();
    layout.dispose();
    core.dispose();
  });
}

const root = document.getElementById('app');
if (!root) throw new Error('#app not found');
void bootstrap(root);
