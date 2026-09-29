import { HashRouter, Navigate, Route, Routes } from 'react-router-dom';
import { Layout } from './components/Layout';
import { TOOLS } from './nav';
import BrowserPluginsPage from './pages/BrowserPluginsPage';
import CleanPage from './pages/CleanPage';
import CookiesPage from './pages/CookiesPage';
import DiskAnalyzerPage from './pages/DiskAnalyzerPage';
import DriveWiperPage from './pages/DriveWiperPage';
import DriverUpdaterPage from './pages/DriverUpdaterPage';
import DuplicateFinderPage from './pages/DuplicateFinderPage';
import HomePage from './pages/HomePage';
import PerformancePage from './pages/PerformancePage';
import RegistryPage from './pages/RegistryPage';
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
  registry: RegistryPage,
  sysinfo: SysInfoPage,
  cookies: CookiesPage,
};

export default function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<HomePage />} />
          <Route path="clean" element={<CleanPage />} />
          <Route path="tools" element={<ToolsPage />} />
          {TOOLS.map((t) => {
            const Page = toolPages[t.id]!;
            return <Route key={t.id} path={`tools/${t.id}`} element={<Page />} />;
          })}
          <Route path="performance" element={<PerformancePage />} />
          <Route path="settings" element={<SettingsPage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
