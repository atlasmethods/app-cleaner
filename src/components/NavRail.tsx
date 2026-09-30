import { NavLink } from 'react-router-dom';
import { t } from '../i18n';
import { TABS, TOOLS } from '../nav';

const linkBase =
  'flex min-h-10 items-center gap-3 rounded-lg px-3 text-sm font-medium no-underline hover:bg-surface-2';

/** Wide layout: the five tabs, with every Tools entry listed under Tools. */
export function NavRail() {
  return (
    <nav
      aria-label="Main"
      data-testid="navrail"
      className="flex h-full w-60 shrink-0 flex-col gap-1 overflow-y-auto overscroll-contain border-r border-line bg-surface p-3"
    >
      <p className="m-0 px-3 pb-2 pt-1 text-base font-semibold tracking-tight">ClearSweep</p>
      <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
        {TABS.map(({ id, path, icon: Icon }) => (
          <li key={id}>
            <NavLink
              to={path}
              end={path === '/'}
              data-testid={`tab-${id}`}
              className={({ isActive }) =>
                `${linkBase} ${isActive ? 'bg-accent/10 text-accent-strong' : 'text-fg'}`
              }
            >
              <Icon size={18} aria-hidden />
              {t(`tab.${id}`)}
            </NavLink>
            {id === 'tools' && (
              <ul
                aria-label="Tools"
                data-testid="navrail-tools"
                className="m-0 mb-1 ml-5 mt-0.5 flex list-none flex-col gap-0.5 border-l border-line p-0 pl-2"
              >
                {TOOLS.map((tool) => (
                  <li key={tool.id}>
                    <NavLink
                      to={tool.path}
                      data-testid={`rail-${tool.id}`}
                      className={({ isActive }) =>
                        `${linkBase} text-[13px] ${isActive ? 'bg-accent/10 text-accent-strong' : 'text-muted'}`
                      }
                    >
                      <tool.icon size={16} aria-hidden />
                      <span className="min-w-0 truncate">{tool.label}</span>
                    </NavLink>
                  </li>
                ))}
              </ul>
            )}
          </li>
        ))}
      </ul>
    </nav>
  );
}
