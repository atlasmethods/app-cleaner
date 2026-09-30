import { useEffect, useRef } from 'react';
import { Outlet, useLocation, useNavigate } from 'react-router-dom';
import { useWideLayout } from '../lib/layout';
import { routeMeta } from '../nav';
import { AppBar } from './AppBar';
import { NavRail } from './NavRail';
import { TabBar } from './TabBar';

/**
 * Compact (< 900px, or forced in Settings): app bar, scrolling content, bottom tab bar.
 * Wide (>= 900px): navigation rail on the left, app bar + a content column of at most 720px.
 * The tab bar is a flex child, not `fixed`, so `sticky bottom-0` action bars in pages sit above
 * it and nothing shifts when the content scrolls or the mobile toolbar collapses.
 */
export function Layout() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const wide = useWideLayout();
  const { title, parent } = routeMeta(pathname);
  const main = useRef<HTMLElement>(null);
  const first = useRef(true);

  useEffect(() => {
    document.title = `${title} - ClearSweep`;
  }, [title]);

  // After a navigation, keyboard focus restarts at the top of the new page.
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    main.current?.focus({ preventScroll: true });
    main.current?.scrollTo?.({ top: 0 });
  }, [pathname]);

  const content = (
    <main
      ref={main}
      tabIndex={-1}
      data-scroll-root
      className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden overscroll-contain outline-none"
    >
      <div className={`mx-auto h-full w-full ${wide ? 'max-w-[720px]' : ''}`}>
        <Outlet />
      </div>
    </main>
  );

  if (wide) {
    return (
      <div data-layout="wide" className="flex h-dvh w-full overflow-hidden bg-bg">
        <NavRail />
        <div className="flex min-w-0 flex-1 flex-col">
          <AppBar title={title} onBack={parent ? () => navigate(parent) : undefined} />
          {content}
        </div>
      </div>
    );
  }
  return (
    <div
      data-layout="compact"
      className="mx-auto flex h-dvh w-full max-w-[480px] flex-col overflow-hidden bg-bg min-[481px]:border-x min-[481px]:border-line"
    >
      <AppBar title={title} onBack={parent ? () => navigate(parent) : undefined} />
      {content}
      <TabBar />
    </div>
  );
}
