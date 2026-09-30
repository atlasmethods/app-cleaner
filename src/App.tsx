import { useSyncExternalStore } from 'react';
import { HashRouter, Route, Routes } from 'react-router-dom';
import { Layout } from './components/Layout';
import { isAuthRequired, subscribeAuth } from './lib/transport';
import { TOOLS } from './nav';
import AuthRequiredPage from './pages/AuthRequiredPage';
import BrowserPluginsPage from './pages/BrowserPluginsPage';
import CleanHistoryPage from './pages/CleanHistoryPage';
import CleanPage from './pages/CleanPage';
import CookiesPage from './pages/CookiesPage';
import DiskAnalyzerPage from './pages/DiskAnalyzerPage';
import DriveWiperPage from './pages/DriveWiperPage';
import DriverUpdaterPage from './pages/DriverUpdaterPage';
import DuplicateFinderPage from './pages/DuplicateFinderPage';
import FileShredderPage from './pages/FileShredderPage';
import HomePage from './pages/HomePage';
import NotFoundPage from './pages/NotFoundPage';
import PerformancePage from './pages/PerformancePage';
import RegistryPage from './pages/RegistryPage';
import SchedulesPage from './pages/SchedulesPage';
import SettingsPage from './pages/SettingsPage';
import SoftwareUpdaterPage from './pages/SoftwareUpdaterPage';
import StartupPage from './pages/StartupPage';
import SysInfoPage from './pages/SysInfoPage';
import SystemRestorePage from './pages/SystemRestorePage';
import ToolsPage from './pages/ToolsPage';
import UninstallPage from './pages/UninstallPage';

const toolPages: Record<string, React.ComponentType> = {
  uninstall: UninstallPage,
  updater: SoftwareUpdaterPage,
  drivers: DriverUpdaterPage,
  startup: StartupPage,
  plugins: BrowserPluginsPage,
  disk: DiskAnalyzerPage,
  duplicates: DuplicateFinderPage,
  restore: SystemRestorePage,
  wiper: DriveWiperPage,
  shredder: FileShredderPage,
  registry: RegistryPage,
  sysinfo: SysInfoPage,
  cookies: CookiesPage,
};

export default function App() {
  const authRequired = useSyncExternalStore(subscribeAuth, isAuthRequired, () => false);
  if (authRequired) return <AuthRequiredPage />;
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<HomePage />} />
          <Route path="clean" element={<CleanPage />} />
          <Route path="clean/history" element={<CleanHistoryPage />} />
          <Route path="tools" element={<ToolsPage />} />
          {TOOLS.map((t) => {
            const Page = toolPages[t.id]!;
            return <Route key={t.id} path={`tools/${t.id}`} element={<Page />} />;
          })}
          <Route path="performance" element={<PerformancePage />} />
          <Route path="settings" element={<SettingsPage />} />
          <Route path="settings/schedules" element={<SchedulesPage />} />
          <Route path="*" element={<NotFoundPage />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
