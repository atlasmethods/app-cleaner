import { NavLink } from 'react-router-dom';
import { TABS } from '../nav';

export function TabBar() {
  return (
    <nav
      aria-label="Main"
      data-testid="tabbar"
      className="fixed bottom-0 left-1/2 z-20 w-full max-w-[480px] -translate-x-1/2 border-t border-line bg-surface pb-[env(safe-area-inset-bottom)]"
    >
      <ul className="m-0 flex list-none p-0">
        {TABS.map(({ id, label, path, icon: Icon }) => (
          <li key={id} className="min-w-0 flex-1">
            <NavLink
              to={path}
              end={path === '/'}
              data-testid={`tab-${id}`}
              className={({ isActive }) =>
                `flex h-14 flex-col items-center justify-center gap-0.5 px-0 text-[10px] font-medium tracking-tight no-underline ${
                  isActive ? 'text-accent-strong' : 'text-muted'
                }`
              }
            >
              <Icon size={20} aria-hidden />
              <span className="max-w-full whitespace-nowrap">{label}</span>
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  );
}
