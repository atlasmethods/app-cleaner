import { Outlet, useLocation, useNavigate } from 'react-router-dom';
import { routeMeta } from '../nav';
import { AppBar } from './AppBar';
import { TabBar } from './TabBar';

export function Layout() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const { title, parent } = routeMeta(pathname);
  return (
    <div className="mx-auto flex h-dvh w-full max-w-[480px] flex-col overflow-hidden bg-bg">
      <AppBar title={title} onBack={parent ? () => navigate(parent) : undefined} />
      <main className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden pb-[calc(3.5rem+env(safe-area-inset-bottom))]">
        <Outlet />
      </main>
      <TabBar />
    </div>
  );
}
