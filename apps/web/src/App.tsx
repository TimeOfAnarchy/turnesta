import { lazy, Suspense, useEffect } from 'react';
import type { ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, Navigate, NavLink, Outlet, Route, Routes, useLocation, useOutletContext, useParams } from 'react-router-dom';
import { api, ApiError, roleLabels, readRecoveryInterval } from './api';
import type { Organization, Role } from './api';
import { Brand, ErrorNotice, Icon, Loading } from './components';
import { useOrganization, useOrganizations } from './queries';
import { LocationsPage } from './LocationsPage';
import { BookingsPage } from './BookingsPage';
import { SettingsPage } from './SettingsPage';
import { TipsPage } from './TipsPage';
import { QueuesPage, QueuePage } from './QueuesPage';

const TeamPage = lazy(() => import('./TeamPage').then(module => ({ default: module.TeamPage })));

export type Workspace = { organization: Organization; role: Role; canManage: boolean; userId: string };
export function useWorkspace() { return useOutletContext<Workspace>(); }

export function App() {
  const session = useQuery({ queryKey: ['session'], queryFn: ({ signal }) => api.session(signal), staleTime: 0, refetchInterval: query => readRecoveryInterval(query.state.error) });
  if (session.isPending) return <div className="entry-state"><Brand/><Loading/></div>;
  if (session.isError) return <div className="entry-state"><Brand/><ErrorNotice error={session.error} retry={() => { void session.refetch(); }}/></div>;
  return <Routes>
    <Route index element={<OrganizationPicker/>}/>
    <Route path="organizations/:tenantId" element={<OrganizationLayout userId={session.data.user_id}/>}>
      <Route index element={<LastSection/>}/>
      <Route path="locations" element={<LocationsPage/>}/>
      <Route path="bookings" element={<BookingsPage/>}/>
      <Route path="settings" element={<SettingsPage/>}/>
      <Route path="team" element={<Suspense fallback={<Loading/>}><TeamPage/></Suspense>}/>
      <Route path="tips" element={<TipsPage/>}/>
      <Route path="queues" element={<QueuesPage/>}/>
      <Route path="queues/:queueId" element={<QueuePage/>}/>
    </Route>
    <Route path="*" element={<Shell><div className="empty-state"><span className="large-icon"><Icon name="grid"/></span><h1>Страница не найдена</h1><p>Вернитесь к своим организациям, чтобы продолжить работу.</p><Link className="button primary" to="/">К организациям <Icon name="arrow"/></Link></div></Shell>}/>
  </Routes>;
}

function Shell({ organization, role, children }: { organization?: Organization; role?: Role; children: ReactNode }) {
  const client = useQueryClient();
  const pathname = useLocation().pathname;
  const isSettings = pathname.endsWith('/settings');
  const isTeam = pathname.endsWith('/team');
  const isTips = pathname.endsWith('/tips');
  const isBookings = pathname.includes('/bookings');
  const isQueue = pathname.includes('/queues');
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: () => { client.clear(); window.location.replace('/login'); },
  });
  const base = organization ? `/organizations/${organization.id}` : '';
  return <div className="app-shell">
    <a href="#main-content" className="skip-link">К содержимому</a>
    <aside className="sidebar" aria-label="Рабочее пространство">
      <Link className="brand-link" to="/" state={pickState} aria-label="Turnesta — организации"><Brand/></Link>
      <p className="brand-caption">Всё идёт своим чередом.</p>
      <div className="sidebar-divider"/>
      {organization ? <>
        <div className="workspace-label">РАБОЧЕЕ ПРОСТРАНСТВО</div>
        <Link className="workspace-switch" to="/" state={pickState} aria-label={`Сменить организацию. Сейчас ${organization.name}`}>
          <span className="workspace-avatar" aria-hidden="true">{[...organization.name][0]?.toUpperCase()}</span>
          <span className="workspace-copy"><strong>{organization.name}</strong><span>Сменить организацию</span></span><Icon name="chevron"/>
        </Link>
        <nav className="main-nav" aria-label="Организация">
          <NavLink to={`${base}/locations`} className={({ isActive }) => isActive || pathname === base ? 'nav-item active' : 'nav-item'}><Icon name="locations"/>Точки</NavLink>
          <NavLink to={`${base}/queues`} className={({ isActive }) => isActive ? 'nav-item active' : 'nav-item'}><Icon name="queue"/>Очереди</NavLink>
          <NavLink to={`${base}/bookings`} className={({ isActive }) => isActive ? 'nav-item active' : 'nav-item'}><Icon name="calendar"/>Запись</NavLink>
          <NavLink to={`${base}/tips`} className={({ isActive }) => isActive ? 'nav-item active' : 'nav-item'}><Icon name="heart"/>Чаевые</NavLink>
          {role === 'owner' && <NavLink to={`${base}/team`} className={({ isActive }) => isActive ? 'nav-item active' : 'nav-item'}><Icon name="users"/>Команда</NavLink>}
          <NavLink to={`${base}/settings`} className={({ isActive }) => isActive ? 'nav-item active' : 'nav-item'}><Icon name="settings"/>Настройки</NavLink>
        </nav>
      </> : <nav className="main-nav" aria-label="Главная"><Link className="nav-item active" to="/"><Icon name="grid"/>Мои организации</Link></nav>}
      <div className="sidebar-bottom">
        <div className="sidebar-note"><span className="note-dot" aria-hidden="true"/><p>Больше порядка.<br/>Больше времени для людей.</p></div>
        <div className="account-row"><span className="account-avatar" aria-hidden="true">В</span><div><strong>Ваш кабинет</strong><span>{role ? roleLabels[role] : 'Рабочее пространство'}</span></div><button type="button" className="icon-button logout-button" onClick={() => logout.mutate()} disabled={logout.isPending} aria-label={logout.isPending ? 'Выходим…' : 'Выйти из Turnesta'} title="Выйти"><Icon name="exit"/></button></div>
        {logout.isError && <ErrorNotice error={logout.error}/>}
      </div>
    </aside>
    <div className="workspace-main">
      <header className="topbar"><div className="breadcrumb"><Link to="/" state={pickState}>Мои организации</Link>{organization && <><span aria-hidden="true">/</span><span className="breadcrumb-organization">{organization.name}</span><span aria-hidden="true">/</span><strong>{isTeam ? 'Команда' : isBookings ? 'Запись' : isSettings ? 'Настройки' : isTips ? 'Чаевые' : isQueue ? 'Очереди' : 'Точки'}</strong></>}</div><span className="topbar-tag">ВАШ БИЗНЕС. ВАШ РИТМ.</span></header>
      <main className="main-content" id="main-content" tabIndex={-1}>{children}</main>
      <footer className="app-footer"><span>Сделано для хорошего сервиса.</span><span>turnesta<span className="orange-dot">•</span></span></footer>
    </div>
  </div>;
}

function OrganizationPicker() {
  const organizations = useOrganizations();
  const items = organizations.data?.pages.flatMap((page) => page.items) ?? [];
  const picking = (useLocation().state as { pick?: boolean } | null)?.pick === true;
  useEffect(() => { document.title = 'Мои организации · Turnesta'; }, []);
  if (!picking && items.length === 1 && !organizations.hasNextPage) return <Navigate to={`/organizations/${items[0].id}`} replace/>;
  return <Shell>
    <div className="page-heading"><div><p className="eyebrow">ВАШЕ РАБОЧЕЕ ПРОСТРАНСТВО</p><h1>Мои организации<span className="heading-dot">.</span></h1><p className="page-intro">Выберите, с какой организацией будем работать.</p></div><span className="heading-decoration" aria-hidden="true"><Icon name="grid"/></span></div>
    {organizations.isPending ? <Loading label="Загружаем организации…"/> : organizations.isError && !items.length ? <ErrorNotice error={organizations.error} retry={() => { void organizations.refetch(); }}/> : items.length === 0 ? <div className="panel empty-state"><span className="large-icon"><Icon name="grid"/></span><h2>Здесь появятся ваши организации</h2><p>Вы уже вошли в Turnesta. Попросите владельца организации добавить вас в команду.</p><button type="button" className="button secondary" onClick={() => { void organizations.refetch(); }}>Проверить доступ</button></div> : <>
      <div className="section-line"><h2>Доступные организации</h2><span className="subtle">Выберите рабочее пространство</span></div>
      <div className="organization-grid">{items.map((organization, index) => <Link key={organization.id} to={`/organizations/${organization.id}`} className="organization-card">
        <div className="organization-card-top"><span className={`organization-mark mark-${index % 3}`} aria-hidden="true">{[...organization.name][0]?.toUpperCase()}</span><span className="role-badge">{roleLabels[organization.role]}</span></div>
        <h3>{organization.name}</h3><p>Управление рабочим пространством</p><div className="organization-card-bottom"><span>Открыть организацию</span><span className="round-arrow"><Icon name="arrow"/></span></div>
      </Link>)}</div>
      {organizations.isError && <ErrorNotice error={organizations.error} retry={() => { void organizations.refetch(); }}/>}
      {organizations.hasNextPage && <div className="pagination"><button className="button secondary" type="button" onClick={() => { void organizations.fetchNextPage(); }} disabled={organizations.isFetchingNextPage}>{organizations.isFetchingNextPage ? 'Загружаем…' : 'Показать ещё организации'}</button></div>}
    </>}
  </Shell>;
}

const pickState = { pick: true };
const sections = ['locations', 'queues', 'bookings', 'tips', 'team', 'settings'];
const sectionKey = (tenant: string) => `turnesta:section:${tenant}`;
// The last opened section is a per-browser convenience; storage may be unavailable.
function rememberSection(tenant: string, section: string) {
  try { window.localStorage.setItem(sectionKey(tenant), section); } catch { /* not persisted */ }
}
function LastSection() {
  const { organization, role } = useWorkspace();
  let section = 'locations';
  try { section = window.localStorage.getItem(sectionKey(organization.id)) ?? section; } catch { /* default section */ }
  if (!sections.includes(section) || (section === 'team' && role !== 'owner')) section = 'locations';
  return <Navigate to={section} replace/>;
}

function OrganizationLayout({ userId }: { userId: string }) {
  const { tenantId = '' } = useParams();
  return <OrganizationWorkspace key={tenantId} tenantId={tenantId} userId={userId}/>;
}

function OrganizationWorkspace({ tenantId, userId }: { tenantId: string; userId: string }) {
  const organization = useOrganization(tenantId);
  const section = useLocation().pathname.split('/')[3];
  useEffect(() => { if (section && sections.includes(section)) rememberSection(tenantId, section); }, [tenantId, section]);
  const organizations = useOrganizations();
  const membership = organizations.data?.pages.flatMap((page) => page.items).find((item) => item.id === tenantId);
  useEffect(() => {
    if (organization.isSuccess && !membership && organizations.hasNextPage && !organizations.isFetching && !organizations.isError) void organizations.fetchNextPage();
  }, [organization.isSuccess, membership, organizations.hasNextPage, organizations.isFetching, organizations.isError, organizations.fetchNextPage]);
  const pending = organization.isPending || organizations.isPending || (!membership && organizations.hasNextPage && !organizations.isError);
  return <Shell organization={organization.data} role={membership?.role}>
    {pending ? <Loading/> : organization.isError ? organization.error instanceof ApiError && [400, 403, 404].includes(organization.error.status) ? <div className="panel empty-state"><span className="large-icon"><Icon name="grid"/></span><h1>Организация недоступна</h1><p>Возможно, ссылка устарела или ваши права изменились. Выберите организацию из доступных вам.</p><Link className="button primary" to="/">К моим организациям <Icon name="arrow"/></Link></div> : <ErrorNotice error={organization.error} retry={() => { void organization.refetch(); }}/> : organizations.isError ? <ErrorNotice error={organizations.error} retry={() => { void organizations.refetch(); }}/> : !membership || !organization.data ? <div className="panel empty-state"><h1>Доступ изменился</h1><p>Вернитесь к списку организаций и выберите рабочее пространство.</p><Link className="button primary" to="/">К организациям</Link></div> : <Outlet context={{ organization: organization.data, role: membership.role, canManage: membership.role === 'owner' || membership.role === 'manager', userId } satisfies Workspace}/>}
  </Shell>;
}
