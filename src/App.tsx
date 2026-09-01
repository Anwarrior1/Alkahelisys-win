import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react';
import {
  Activity,
  AlertTriangle,
  ArrowDownLeft,
  ArrowUpLeft,
  Banknote,
  BarChart3,
  Bell,
  Building2,
  CalendarDays,
  Car,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDollarSign,
  Clock3,
  DatabaseBackup,
  Download,
  Eye,
  EyeOff,
  FileText,
  History,
  KeyRound,
  LayoutDashboard,
  LoaderCircle,
  LockKeyhole,
  LogOut,
  MapPin,
  Menu,
  Minus,
  Moon,
  Pencil,
  Phone,
  Plus,
  Printer,
  ReceiptText,
  RefreshCw,
  Save,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  Sun,
  Trash2,
  Undo2,
  Upload,
  UserPlus,
  UsersRound,
  WalletCards,
  X,
} from 'lucide-react';
import { api, ApiError } from './api';
import type {
  AppSettings,
  AuditLog,
  AuthUser,
  BackupRecord,
  DashboardData,
  DateRange,
  ExpenseAllocation,
  ExpenseRecord,
  FinanceOverview,
  FinancialReport,
  OperationalReport,
  OvernightCar,
  PaymentRecord,
  PayrollEmployee,
  PayrollSummary,
  Role,
  SalaryDeduction,
  SalaryWithdrawal,
  Showroom,
  ShowroomDebtProfile,
  ShowroomDebtSummary,
  Wash,
  Worker,
  WorkerWithdrawalReturnLedger,
} from './types';

type Theme = 'light' | 'dark';
const DASHBOARD_REFRESH_EVENT = 'alkaheli:dashboard-refresh';
const FINANCIAL_REFRESH_EVENT = 'alkaheli:financial-refresh';

function refreshDashboard() {
  window.dispatchEvent(new Event(DASHBOARD_REFRESH_EVENT));
}

function refreshFinancialViews() {
  window.dispatchEvent(new Event(FINANCIAL_REFRESH_EVENT));
  refreshDashboard();
}

type View =
  | 'dashboard'
  | 'washes'
  | 'paidCars'
  | 'overnight'
  | 'workers'
  | 'showrooms'
  | 'showroomDebts'
  | 'reports'
  | 'finance'
  | 'salaries'
  | 'expenses'
  | 'settings'
  | 'audit'
  | 'backup';

type ToastMessage = { tone: 'success' | 'error' | 'info'; text: string } | null;

const BUSINESS_NAME = 'مركز الكحيلي';
const DEBT_REPORT_BUSINESS_NAME = 'مركز الكحيلي لغسيل السيارات';
const UI_LOCALE = 'ar-LY-u-nu-latn';
const LATIN_NUMBERING_SYSTEM = 'latn';
const BUSINESS_TIME_ZONE = 'Africa/Tripoli';
const WORKING_DATE_KEY = 'alkaheli.working-date';
const ALL_VIEWS: View[] = [
  'dashboard',
  'washes',
  'paidCars',
  'overnight',
  'workers',
  'showrooms',
  'showroomDebts',
  'reports',
  'finance',
  'salaries',
  'expenses',
  'settings',
  'audit',
  'backup',
];
type PermissionCode = 'operational.read' | 'operational.write' | 'financial.manage' | 'dashboard.daily_revenue.read' | 'worker.daily_value.manage' | 'settings.manage' | 'users.manage' | 'audit.read' | 'backup.manage';

const PERMISSION_OPTIONS: { code: PermissionCode; name: string; description: string }[] = [
  { code: 'operational.read', name: 'عرض أقسام التشغيل', description: 'لوحة المتابعة وعمليات الغسيل والعمال والمعارض والتقارير التشغيلية.' },
  { code: 'operational.write', name: 'إضافة وتعديل بيانات التشغيل', description: 'تسجيل عمليات الغسيل وتعديلها وإدارة العمال والمعارض.' },
  { code: 'financial.manage', name: 'عرض وإدارة البيانات المالية', description: 'المركز المالي والمصروفات والديون والتقارير والتفاصيل المالية.' },
  { code: 'dashboard.daily_revenue.read', name: 'عرض الإيراد اليومي', description: 'إظهار بطاقة إيراد اليوم وقيمتها في لوحة المتابعة.' },
  { code: 'worker.daily_value.manage', name: 'إدارة القيمة اليومية للعامل', description: 'عرض وتحديث القيمة اليومية داخل ملفات العمال.' },
  { code: 'settings.manage', name: 'إدارة إعدادات المركز', description: 'اسم المركز والعملة ونسبة العمولة الافتراضية.' },
  { code: 'users.manage', name: 'إدارة حسابات المستخدمين', description: 'إنشاء حسابات الموظفين وتعديلها وتعطيلها.' },
  { code: 'audit.read', name: 'عرض سجل التدقيق', description: 'مراجعة سجل الإجراءات والتغييرات المهمة.' },
  { code: 'backup.manage', name: 'إدارة النسخ الاحتياطية', description: 'إنشاء النسخ الاحتياطية واستعادتها.' },
];

const dashboardFallback: DashboardData = {
  operational: { cars_today: 0, cars_this_month: 0, active_workers: 0, recent_washes: [] },
};

function managerOf(user: AuthUser) {
  return user.role.toLowerCase() === 'manager';
}

function hasPermission(user: AuthUser, permission: PermissionCode) {
  return managerOf(user) || Boolean(user.permissions?.includes(permission));
}

function canAccessView(user: AuthUser, view: View) {
  if (view === 'overnight' || view === 'paidCars') return hasPermission(user, 'operational.read');
  if (['dashboard', 'washes', 'workers', 'showrooms', 'reports'].includes(view)) return hasPermission(user, 'operational.read');
  if (view === 'finance' || view === 'showroomDebts' || view === 'salaries' || view === 'expenses') return hasPermission(user, 'financial.manage');
  if (view === 'settings') return hasPermission(user, 'settings.manage') || hasPermission(user, 'users.manage');
  if (view === 'audit') return hasPermission(user, 'audit.read');
  if (view === 'backup') return hasPermission(user, 'backup.manage');
  return false;
}

function firstAccessibleView(user: AuthUser): View {
  return ALL_VIEWS.find((candidate) => canAccessView(user, candidate)) ?? 'dashboard';
}

function westernDigits(value: string) {
  return value
    .replace(/[\u0660-\u0669]/g, (digit) => String(digit.charCodeAt(0) - 0x660))
    .replace(/[\u06f0-\u06f9]/g, (digit) => String(digit.charCodeAt(0) - 0x6f0));
}

function safeNumber(value: number | string | undefined | null) {
  if (typeof value === 'number') return Number.isFinite(value) ? value : 0;
  const normalized = westernDigits(String(value ?? '0'))
    .replace(/[\u066c,]/g, '')
    .replace(/\u066b/g, '.');
  const parsed = Number(normalized);
  return Number.isFinite(parsed) ? parsed : 0;
}

function formatNumber(value: number | string | undefined | null, options?: Intl.NumberFormatOptions) {
  return westernDigits(new Intl.NumberFormat(UI_LOCALE, {
    ...options,
    numberingSystem: LATIN_NUMBERING_SYSTEM,
  }).format(safeNumber(value)));
}

function money(value: number | string | undefined | null, currency = 'د.ل') {
  return `${formatNumber(value, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })} ${currency}`;
}

function dateFormat(value?: string | null, withTime = false) {
  if (!value) return '—';
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return westernDigits(value);
  return westernDigits(new Intl.DateTimeFormat(UI_LOCALE, {
    dateStyle: 'medium',
    ...(withTime ? { timeStyle: 'short' as const } : {}),
    numberingSystem: LATIN_NUMBERING_SYSTEM,
  }).format(parsed));
}

function businessDateKey(date = new Date()) {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: BUSINESS_TIME_ZONE,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).formatToParts(date);
  const value = (type: 'year' | 'month' | 'day') => parts.find((part) => part.type === type)?.value ?? '';
  return `${value('year')}-${value('month')}-${value('day')}`;
}

function shiftDateKey(dateKey: string, days: number) {
  const [year, month, day] = dateKey.split('-').map(Number);
  const shifted = new Date(Date.UTC(year, month - 1, day + days, 12));
  return shifted.toISOString().slice(0, 10);
}

function selectedDateRange(dateKey: string): DateRange {
  return { from: dateKey, to: dateKey };
}

function initialWorkingDate(today: string) {
  try {
    const stored = window.sessionStorage.getItem(WORKING_DATE_KEY);
    return stored && /^\d{4}-\d{2}-\d{2}$/.test(stored) && stored <= today ? stored : today;
  } catch {
    return today;
  }
}

function workingDateFormat(dateKey: string) {
  const date = new Date(`${dateKey}T12:00:00Z`);
  return westernDigits(new Intl.DateTimeFormat(UI_LOCALE, {
    dateStyle: 'medium',
    numberingSystem: LATIN_NUMBERING_SYSTEM,
    timeZone: BUSINESS_TIME_ZONE,
  }).format(date));
}

function dateInputValue(date = new Date()) {
  const adjusted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return adjusted.toISOString().slice(0, 10);
}

function monthInputValue(date = new Date()) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`;
}

function dateTimeInputValue(date = new Date()) {
  const adjusted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return adjusted.toISOString().slice(0, 16);
}

function friendlyError(error: unknown) {
  return error instanceof ApiError ? error.message : 'حدث خطأ غير متوقع. يرجى المحاولة مرة أخرى.';
}

function viewFromHash(): View {
  const candidate = window.location.hash.replace('#/', '').replace('#', '') as View;
  return ALL_VIEWS.includes(candidate) ? candidate : 'dashboard';
}

function BrandMark({ compact = false }: { compact?: boolean }) {
  return (
    <div className={`brand ${compact ? 'brand--compact' : ''}`}>
      <div className="brand__mark" aria-hidden="true">
        <Car size={compact ? 19 : 23} strokeWidth={2.2} />
        <span className="brand__drop">✦</span>
      </div>
      {!compact && (
        <div className="brand__text">
          <span>{BUSINESS_NAME}</span>
          <small>لغسيل السيارات</small>
        </div>
      )}
    </div>
  );
}

function PageHeader({ eyebrow, title, description, actions }: { eyebrow?: string; title: string; description?: string; actions?: ReactNode }) {
  return (
    <div className="page-header">
      <div>
        {eyebrow && <p className="eyebrow">{eyebrow}</p>}
        <h1>{title}</h1>
        {description && <p className="page-header__description">{description}</p>}
      </div>
      {actions && <div className="page-header__actions">{actions}</div>}
    </div>
  );
}

function SectionCard({ children, className = '', title, subtitle, action }: { children: ReactNode; className?: string; title?: string; subtitle?: string; action?: ReactNode }) {
  return (
    <section className={`glass-card section-card ${className}`}>
      {(title || action) && (
        <div className="section-card__header">
          <div>
            {title && <h2>{title}</h2>}
            {subtitle && <p>{subtitle}</p>}
          </div>
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

function Button({
  children,
  type = 'button',
  variant = 'primary',
  icon,
  onClick,
  disabled = false,
  className = '',
}: {
  children: ReactNode;
  type?: 'button' | 'submit' | 'reset';
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  icon?: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <button className={`button button--${variant} ${className}`} type={type} onClick={onClick} disabled={disabled}>
      {icon}
      <span>{children}</span>
    </button>
  );
}

function LoadingBlock({ label = 'جارٍ تحميل البيانات...' }: { label?: string }) {
  return (
    <div className="loading-block">
      <LoaderCircle size={22} className="spin" />
      <span>{label}</span>
    </div>
  );
}

function EmptyState({ icon = <Sparkles size={28} />, title, description, action }: { icon?: ReactNode; title: string; description?: string; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <div className="empty-state__icon">{icon}</div>
      <h3>{title}</h3>
      {description && <p>{description}</p>}
      {action && <div>{action}</div>}
    </div>
  );
}

function MetricCard({ label, value, note, icon, tone = 'blue' }: { label: string; value: ReactNode; note?: string; icon: ReactNode; tone?: 'blue' | 'teal' | 'violet' | 'amber' }) {
  return (
    <div className={`metric-card metric-card--${tone}`}>
      <div className="metric-card__icon">{icon}</div>
      <div className="metric-card__content">
        <span>{label}</span>
        <strong>{value}</strong>
        {note && <small>{note}</small>}
      </div>
    </div>
  );
}

function StatusBadge({ children, tone = 'neutral' }: { children: ReactNode; tone?: 'neutral' | 'success' | 'warning' | 'danger' | 'info' }) {
  return <span className={`status-badge status-badge--${tone}`}>{children}</span>;
}

function FormField({ label, children, hint, required = false }: { label: string; children: ReactNode; hint?: string; required?: boolean }) {
  return (
    <label className="form-field">
      <span className="form-field__label">{label}{required && <b> *</b>}</span>
      {children}
      {hint && <small>{hint}</small>}
    </label>
  );
}

function Toast({ message, onDismiss }: { message: ToastMessage; onDismiss: () => void }) {
  useEffect(() => {
    if (!message) return undefined;
    const timeout = window.setTimeout(onDismiss, 5000);
    return () => window.clearTimeout(timeout);
  }, [message, onDismiss]);

  if (!message) return null;
  const Icon = message.tone === 'success' ? CheckCircle2 : message.tone === 'error' ? AlertTriangle : Bell;
  return (
    <div className={`toast toast--${message.tone}`} role="status">
      <Icon size={19} />
      <span>{message.text}</span>
      <button onClick={onDismiss} aria-label="إغلاق التنبيه"><X size={17} /></button>
    </div>
  );
}

export default function App() {
  const [user, setUser] = useState<AuthUser | null>(null);
  const [booting, setBooting] = useState(true);
  const [setupRequired, setSetupRequired] = useState(false);
  const [bootError, setBootError] = useState<string | null>(null);
  const [theme, setTheme] = useState<Theme>('light');

  useEffect(() => {
    document.documentElement.lang = UI_LOCALE;
    document.documentElement.dir = 'rtl';
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    let mounted = true;
    const boot = async () => {
      try {
        const status = await api.setupStatus();
        const needsSetup = status.setup_required === true || status.configured === false || status.initialized === false || status.has_users === false;
        if (!mounted) return;
        setSetupRequired(needsSetup);
        if (!needsSetup && api.getToken()) {
          const current = await api.me();
          if (!mounted) return;
          setUser(current);
          if (current.theme === 'dark' || current.theme === 'light') setTheme(current.theme);
        }
      } catch (error) {
        if (!mounted) return;
        api.setToken(null);
        setBootError(friendlyError(error));
      } finally {
        if (mounted) setBooting(false);
      }
    };
    void boot();
    return () => { mounted = false; };
  }, []);

  const completeAuthentication = (result: { token: string; user: AuthUser }) => {
    api.setToken(result.token);
    setUser(result.user);
    setSetupRequired(false);
    setBootError(null);
    if (result.user.theme === 'dark' || result.user.theme === 'light') setTheme(result.user.theme);
  };

  const changeTheme = (next: Theme) => {
    setTheme(next);
    if (user) void api.updateTheme(next).catch(() => undefined);
  };

  const signOut = async () => {
    try { await api.logout(); } catch { /* Local session is still safely cleared. */ }
    api.setToken(null);
    setUser(null);
    window.location.hash = '#/dashboard';
  };

  if (booting) {
    return (
      <div className="boot-screen">
        <BrandMark />
        <LoadingBlock label="جارٍ تجهيز مركز العمل..." />
      </div>
    );
  }

  if (!user) {
    return (
      <AuthScreen
        setupRequired={setupRequired}
        initialError={bootError}
        onAuthenticated={completeAuthentication}
        onThemeChange={changeTheme}
        theme={theme}
      />
    );
  }

  return <AppShell user={user} theme={theme} onThemeChange={changeTheme} onLogout={signOut} />;
}

function AuthScreen({
  setupRequired,
  initialError,
  onAuthenticated,
  onThemeChange,
  theme,
}: {
  setupRequired: boolean;
  initialError: string | null;
  onAuthenticated: (result: { token: string; user: AuthUser }) => void;
  onThemeChange: (theme: Theme) => void;
  theme: Theme;
}) {
  const [isSetup, setIsSetup] = useState(setupRequired);
  const [username, setUsername] = useState('');
  const [fullName, setFullName] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState(initialError);
  const [saving, setSaving] = useState(false);

  useEffect(() => setIsSetup(setupRequired), [setupRequired]);
  useEffect(() => setError(initialError), [initialError]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    if (isSetup && password !== confirmPassword) {
      setError('تأكيد كلمة المرور لا يطابق كلمة المرور.');
      return;
    }
    setSaving(true);
    try {
      const result = isSetup
        ? await api.initialManager({ full_name: fullName.trim(), username: username.trim(), password, confirm_password: confirmPassword })
        : await api.login({ username: username.trim(), password });
      onAuthenticated(result);
    } catch (requestError) {
      setError(friendlyError(requestError));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="auth-page">
      <div className="auth-page__orb auth-page__orb--one" />
      <div className="auth-page__orb auth-page__orb--two" />
      <header className="auth-page__header">
        <button className="theme-toggle" onClick={() => onThemeChange(theme === 'light' ? 'dark' : 'light')} aria-label="تبديل المظهر">
          {theme === 'light' ? <Moon size={19} /> : <Sun size={19} />}
        </button>
      </header>
      <main className="auth-layout">
        <section className="auth-card glass-card">
          <div className="auth-card__brand">
            <BrandMark />
          </div>
          <div className="auth-card__heading">
            <div className="auth-card__icon"><KeyRound size={23} /></div>
            <div>
              {isSetup && <p className="eyebrow">تهيئة النظام</p>}
              <h2>{isSetup ? 'إنشاء حساب المدير الأول' : 'تسجيل الدخول'}</h2>
              {isSetup && <span>أنشئ حساب الإدارة الأول للبدء.</span>}
            </div>
          </div>
          {error && <div className="inline-alert inline-alert--error"><AlertTriangle size={18} />{error}</div>}
          <form onSubmit={submit} className="auth-form">
            {isSetup && (
              <FormField label="الاسم الكامل" required>
                <input value={fullName} onChange={(event) => setFullName(event.target.value)} placeholder="مثال: أحمد الكحيلي" required autoComplete="name" />
              </FormField>
            )}
            <FormField label="اسم المستخدم" required>
              <input value={username} onChange={(event) => setUsername(event.target.value)} placeholder="أدخل اسم المستخدم" required autoComplete="username" dir="auto" />
            </FormField>
            <FormField label="كلمة المرور" required hint={isSetup ? 'استخدم كلمة مرور قوية لا تقل عن 8 أحرف.' : undefined}>
              <div className="password-input">
                <input value={password} onChange={(event) => setPassword(event.target.value)} type={showPassword ? 'text' : 'password'} placeholder="أدخل كلمة المرور" required minLength={isSetup ? 8 : undefined} autoComplete={isSetup ? 'new-password' : 'current-password'} dir="ltr" />
                <button type="button" onClick={() => setShowPassword((current) => !current)} aria-label={showPassword ? 'إخفاء كلمة المرور' : 'إظهار كلمة المرور'}>{showPassword ? <EyeOff size={18} /> : <Eye size={18} />}</button>
              </div>
            </FormField>
            {isSetup && (
              <FormField label="تأكيد كلمة المرور" required>
                <input value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} type="password" placeholder="أعد إدخال كلمة المرور" required minLength={8} autoComplete="new-password" dir="ltr" />
              </FormField>
            )}
            <Button type="submit" className="button--full" disabled={saving} icon={saving ? <LoaderCircle size={18} className="spin" /> : <ChevronLeft size={18} />}>
              {saving ? 'جارٍ التحقق...' : isSetup ? 'إنشاء الحساب' : 'تسجيل الدخول'}
            </Button>
          </form>
        </section>
      </main>
    </div>
  );
}

function AppShell({ user, theme, onThemeChange, onLogout }: { user: AuthUser; theme: Theme; onThemeChange: (theme: Theme) => void; onLogout: () => void }) {
  const isManager = managerOf(user);
  const canRead = hasPermission(user, 'operational.read');
  const canWrite = hasPermission(user, 'operational.write');
  const canFinancial = hasPermission(user, 'financial.manage');
  const canSettings = hasPermission(user, 'settings.manage');
  const canUsers = hasPermission(user, 'users.manage');
  const canAudit = hasPermission(user, 'audit.read');
  const canBackup = hasPermission(user, 'backup.manage');
  const [view, setView] = useState<View>(viewFromHash());
  const [sideOpen, setSideOpen] = useState(false);
  const [toast, setToast] = useState<ToastMessage>(null);
  const [profileOpen, setProfileOpen] = useState(false);
  const [avatarVersion, setAvatarVersion] = useState(0);
  const [sidebarExpanded, setSidebarExpanded] = useState(false);
  const [actualToday, setActualToday] = useState(() => businessDateKey());
  const [selectedDate, setSelectedDate] = useState(() => initialWorkingDate(businessDateKey()));

  useEffect(() => {
    try { window.sessionStorage.setItem(WORKING_DATE_KEY, selectedDate); } catch { /* Storage is optional; in-memory state remains global. */ }
  }, [selectedDate]);

  useEffect(() => {
    let knownToday = actualToday;
    const syncBusinessDate = () => {
      const nextToday = businessDateKey();
      if (nextToday === knownToday) return;
      setSelectedDate((current) => current === knownToday || current > nextToday ? nextToday : current);
      knownToday = nextToday;
      setActualToday(nextToday);
    };
    syncBusinessDate();
    const timer = window.setInterval(syncBusinessDate, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const selectView = (next: View) => {
    if (!canAccessView(user, next)) {
      setToast({ tone: 'error', text: 'لا تملك صلاحية الوصول إلى هذه الصفحة.' });
      next = firstAccessibleView(user);
    }
    setView(next);
    window.location.hash = `#/${next}`;
    setSideOpen(false);
  };

  useEffect(() => {
    const handler = () => {
      const requested = viewFromHash();
      if (!canAccessView(user, requested)) {
        const fallback = firstAccessibleView(user);
        setView(fallback);
        window.location.hash = `#/${fallback}`;
        return;
      }
      setView(requested);
    };
    window.addEventListener('hashchange', handler);
    handler();
    return () => window.removeEventListener('hashchange', handler);
  }, [user]);

  const notify = (message: ToastMessage) => setToast(message);
  const name = user.full_name || user.username;

  return (
    <div className={`app-shell ${sidebarExpanded ? 'sidebar-expanded' : 'sidebar-collapsed'}`}>
      <div className={`sidebar-backdrop ${sideOpen ? 'is-visible' : ''}`} onClick={() => setSideOpen(false)} />
      <aside className={`sidebar ${sidebarExpanded ? 'is-expanded' : 'is-collapsed'} ${sideOpen ? 'is-open' : ''}`}>
        <div className="sidebar__brand">
          <BrandMark />
          <button type="button" className="sidebar__toggle" onClick={() => setSidebarExpanded((current) => !current)} aria-expanded={sidebarExpanded} aria-label={sidebarExpanded ? 'طي القائمة الجانبية' : 'توسيع القائمة الجانبية'} title={sidebarExpanded ? 'طي القائمة' : 'توسيع القائمة'}>
            <ChevronLeft size={18} />
          </button>
        </div>
        <nav className="sidebar__nav" aria-label="التنقل الرئيسي">
          <NavGroup label=" ">
            {canRead && <><NavButton active={view === 'dashboard'} onClick={() => selectView('dashboard')} icon={<LayoutDashboard size={19} />}>لوحة المتابعة</NavButton>
            <NavButton active={view === 'washes'} onClick={() => selectView('washes')} icon={<Car size={19} />}>عمليات الغسيل</NavButton>
            <NavButton active={view === 'paidCars'} onClick={() => selectView('paidCars')} icon={<CheckCircle2 size={19} />}>السيارات الخالصة</NavButton>
            {canRead && <NavButton active={view === 'overnight'} onClick={() => selectView('overnight')} icon={<Moon size={19} />}>سيارات المبيت</NavButton>}
            <NavButton active={view === 'workers'} onClick={() => selectView('workers')} icon={<UsersRound size={19} />}>العمال</NavButton>
            <NavButton active={view === 'showrooms'} onClick={() => selectView('showrooms')} icon={<Building2 size={19} />}>المعارض</NavButton></>}
          </NavGroup>
          {(canRead || canFinancial || canSettings || canUsers || canAudit || canBackup) && (
            <NavGroup label="الإدارة والمالية">
              {canRead && <NavButton active={view === 'reports'} onClick={() => selectView('reports')} icon={<BarChart3 size={19} />}>التقارير</NavButton>}
              {canFinancial && <><NavButton active={view === 'finance'} onClick={() => selectView('finance')} icon={<WalletCards size={19} />}>المركز المالي</NavButton>
              <NavButton active={view === 'showroomDebts'} onClick={() => selectView('showroomDebts')} icon={<Building2 size={19} />}>ديون المعارض</NavButton>
              <NavButton active={view === 'salaries'} onClick={() => selectView('salaries')} icon={<Banknote size={19} />}>المرتبات</NavButton>
              <NavButton active={view === 'expenses'} onClick={() => selectView('expenses')} icon={<ReceiptText size={19} />}>المصروفات</NavButton></>}
              {(canSettings || canUsers) && <NavButton active={view === 'settings'} onClick={() => selectView('settings')} icon={<Settings size={19} />}>الإعدادات</NavButton>}
              {canAudit && <NavButton active={view === 'audit'} onClick={() => selectView('audit')} icon={<History size={19} />}>سجل التدقيق</NavButton>}
              {canBackup && <NavButton active={view === 'backup'} onClick={() => selectView('backup')} icon={<DatabaseBackup size={19} />}>النسخ الاحتياطي</NavButton>}
            </NavGroup>
          )}
        </nav>
        <div className="sidebar__footer">
          <button type="button" className="user-summary" onClick={() => setProfileOpen(true)} aria-label="فتح ملف الحساب">
            <ProfileAvatar user={user} refreshKey={avatarVersion} />
            <div><strong>{name}</strong></div>
          </button>
          <button className="signout-button" onClick={onLogout}><LogOut size={18} /> تسجيل الخروج</button>
        </div>
      </aside>
      <div className="app-shell__main">
        <header className="topbar">
          <button className="icon-button topbar__menu" onClick={() => { setSidebarExpanded(true); setSideOpen(true); }} aria-label="فتح القائمة"><Menu size={22} /></button>
          <div className="topbar__utility">
            <div className="topbar__role">{isManager ? 'حساب مدير النظام' : 'حساب موظف التشغيل'}</div>
            <div className="topbar__welcome"><p>مرحبًا، {name}</p></div>
            <div className="topbar__actions">
              <div className="topbar__date" aria-label="تاريخ العمل المحدد">
                <button type="button" className="topbar__date-nav" onClick={() => setSelectedDate((current) => shiftDateKey(current, -1))} aria-label="اليوم السابق" title="اليوم السابق"><ChevronRight size={15} /></button>
                <CalendarDays className="topbar__date-calendar" size={17} />
                <span className="topbar__date-value">{workingDateFormat(selectedDate)}</span>
                <button type="button" className="topbar__date-nav" disabled={selectedDate >= actualToday} onClick={() => setSelectedDate((current) => { const next = shiftDateKey(current, 1); return next > actualToday ? actualToday : next; })} aria-label="اليوم التالي" title={selectedDate >= actualToday ? 'هذا هو تاريخ اليوم' : 'اليوم التالي'}><ChevronLeft size={15} /></button>
              </div>
              <button className="theme-toggle" onClick={() => onThemeChange(theme === 'light' ? 'dark' : 'light')} aria-label="تبديل وضع المظهر">
                {theme === 'light' ? <Moon size={19} /> : <Sun size={19} />}
              </button>
            </div>
          </div>
        </header>
        <main className="workspace">
          <ViewRouter view={view} user={user} selectedDate={selectedDate} canWrite={canWrite} canFinancial={canFinancial} navigate={selectView} notify={notify} />
        </main>
      </div>
      <Toast message={toast} onDismiss={() => setToast(null)} />
      {profileOpen && <UserProfileModal user={user} onClose={() => setProfileOpen(false)} onChanged={() => setAvatarVersion((current) => current + 1)} onNotify={notify} />}
    </div>
  );
}

function NavGroup({ label, children }: { label: string; children: ReactNode }) {
  return <div className={`nav-group ${label === 'الإدارة والمالية' ? 'nav-group--management' : ''}`}><p>{label}</p>{children}</div>;
}

function ProfileAvatar({ user, refreshKey = 0 }: { user: AuthUser; refreshKey?: number }) {
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const name = user.full_name || user.username;
  useEffect(() => {
    let mounted = true;
    let objectUrl: string | null = null;
    setImageUrl(null);
    void api.profilePicture(user.id).then((blob) => {
      if (!mounted || !blob) return;
      objectUrl = URL.createObjectURL(blob);
      setImageUrl(objectUrl);
    }).catch(() => undefined);
    return () => { mounted = false; if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [user.id, refreshKey]);
  return <div className="avatar">{imageUrl ? <img src={imageUrl} alt={name} /> : name.slice(0, 1)}</div>;
}

function UserProfileModal({ user, onClose, onChanged, onNotify }: { user: AuthUser; onClose: () => void; onChanged: () => void; onNotify: (message: ToastMessage) => void }) {
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState(false);
  useEffect(() => () => { if (previewUrl) URL.revokeObjectURL(previewUrl); }, [previewUrl]);
  const chooseFile = (file: File | undefined) => {
    if (!file) return;
    if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type)) { onNotify({ tone: 'error', text: 'اختر صورة بصيغة JPG أو PNG أو WebP.' }); return; }
    if (file.size > 5 * 1024 * 1024) { onNotify({ tone: 'error', text: 'حجم الصورة يجب ألا يتجاوز 5 ميغابايت.' }); return; }
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    setSelectedFile(file);
    setPreviewUrl(URL.createObjectURL(file));
  };
  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!selectedFile) return;
    setSaving(true);
    try { await api.uploadProfilePicture(user.id, selectedFile); onChanged(); onNotify({ tone: 'success', text: 'تم تحديث صورة الحساب.' }); onClose(); }
    catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
    finally { setSaving(false); }
  };
  const remove = async () => {
    setRemoving(true);
    try { await api.deleteProfilePicture(user.id); onChanged(); onNotify({ tone: 'success', text: 'تمت إزالة صورة الحساب والعودة إلى الصورة الافتراضية.' }); onClose(); }
    catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
    finally { setRemoving(false); }
  };
  return <Modal title="ملف الحساب" subtitle="حدّث صورتك الشخصية لتظهر في منطقة الحساب." onClose={onClose}>
    <form className="entry-form account-profile-form" onSubmit={save}>
      <div className="account-profile-preview">{previewUrl ? <img src={previewUrl} alt="معاينة صورة الحساب" /> : <ProfileAvatar user={user} />}</div>
      <div className="account-profile-info"><strong>{user.full_name || user.username}</strong><span>{user.username}</span></div>
      <FormField label="صورة الحساب" hint="JPG أو PNG أو WebP — بحد أقصى 5 ميغابايت."><input type="file" accept="image/jpeg,image/png,image/webp" onChange={(event) => chooseFile(event.target.files?.[0])} /></FormField>
      <div className="modal-actions"><Button type="button" variant="ghost" onClick={() => void remove()} disabled={removing || saving}>{removing ? 'جارٍ الإزالة...' : 'إزالة صورة الحساب'}</Button><Button type="submit" disabled={!selectedFile || saving || removing} icon={<Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ الصورة'}</Button></div>
    </form>
  </Modal>;
}

function NavButton({ active, onClick, icon, children }: { active: boolean; onClick: () => void; icon: ReactNode; children: ReactNode }) {
  const label = typeof children === 'string' ? children : undefined;
  return <button className={`nav-button ${active ? 'is-active' : ''}`} onClick={onClick} aria-label={label} title={label}>{icon}<span>{children}</span>{active && <ChevronLeft size={16} />}</button>;
}

function ViewRouter({ view, user, selectedDate, canWrite, canFinancial, navigate, notify }: { view: View; user: AuthUser; selectedDate: string; canWrite: boolean; canFinancial: boolean; navigate: (view: View) => void; notify: (message: ToastMessage) => void }) {
  if (!canAccessView(user, view)) return <AccessDenied onBack={() => navigate(firstAccessibleView(user))} />;
  switch (view) {
    case 'dashboard': return <DashboardView selectedDate={selectedDate} isManager={managerOf(user)} canFinancial={canFinancial} canWrite={canWrite} navigate={navigate} />;
    case 'washes': return <WashesView selectedDate={selectedDate} isManager={managerOf(user)} canWrite={canWrite} onNotify={notify} />;
    case 'paidCars': return <PaidCarsView selectedDate={selectedDate} isManager={managerOf(user)} canWrite={canWrite} onNotify={notify} />;
    case 'overnight': return <OvernightCarsView selectedDate={selectedDate} canWrite={canWrite} onNotify={notify} />;
    case 'workers': return <WorkersView selectedDate={selectedDate} currentUserId={user.id} isManager={managerOf(user)} canWrite={canWrite} canFinancial={canFinancial} onNotify={notify} />;
    case 'showrooms': return <ShowroomsView selectedDate={selectedDate} canFinancial={canFinancial} canWrite={canWrite} onNotify={notify} />;
    case 'showroomDebts': return <ShowroomDebtsView selectedDate={selectedDate} reportIssuer={user.full_name || user.username} onNotify={notify} />;
    case 'reports': return <ReportsView selectedDate={selectedDate} isManager={canFinancial} />;
    case 'finance': return <FinanceView selectedDate={selectedDate} onNotify={notify} />;
    case 'salaries': return <SalariesView selectedDate={selectedDate} onNotify={notify} />;
    case 'expenses': return <ExpensesView selectedDate={selectedDate} onNotify={notify} />;
    case 'settings': return <SettingsView user={user} canSettings={hasPermission(user, 'settings.manage')} canUsers={hasPermission(user, 'users.manage')} onNotify={notify} />;
    case 'audit': return <AuditView selectedDate={selectedDate} />;
    case 'backup': return <BackupView selectedDate={selectedDate} onNotify={notify} />;
    default: return <DashboardView selectedDate={selectedDate} isManager={managerOf(user)} canFinancial={canFinancial} canWrite={canWrite} navigate={navigate} />;
  }
}

function AccessDenied({ onBack }: { onBack: () => void }) {
  return (
    <div className="access-denied glass-card">
      <div><LockKeyhole size={34} /></div>
      <h1>الوصول محمي</h1>
      <p>هذه الصفحة غير مفعّلة ضمن صلاحيات حسابك، ولا يتم تحميل بياناتها.</p>
      <Button onClick={onBack} icon={<ChevronLeft size={18} />}>العودة إلى لوحة المتابعة</Button>
    </div>
  );
}

function DashboardView({ selectedDate, isManager, canFinancial, canWrite, navigate }: { selectedDate: string; isManager: boolean; canFinancial: boolean; canWrite: boolean; navigate: (view: View) => void }) {
  const [data, setData] = useState<DashboardData>(dashboardFallback);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const loadSequence = useRef(0);

  const load = async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    setError(null);
    try {
      const nextData = await api.dashboard(selectedDate);
      if (sequence === loadSequence.current) setData(nextData);
    } catch (requestError) {
      if (sequence === loadSequence.current) setError(friendlyError(requestError));
    } finally {
      if (sequence === loadSequence.current) setLoading(false);
    }
  };

  useEffect(() => {
    const handleRefresh = () => { void load(); };
    window.addEventListener(DASHBOARD_REFRESH_EVENT, handleRefresh);
    void load();
    return () => window.removeEventListener(DASHBOARD_REFRESH_EVENT, handleRefresh);
  }, [selectedDate]);
  const recent = data.operational?.recent_washes ?? [];
  const finance = data.financial;

  return (
    <>
      <PageHeader eyebrow="لوحة المتابعة" title="عمليات اليوم" description={canFinancial ? `بيانات التشغيل والمالية ليوم ${workingDateFormat(selectedDate)}.` : `بيانات التشغيل ليوم ${workingDateFormat(selectedDate)}.`} actions={canWrite ? <Button variant="secondary" onClick={() => navigate('washes')} icon={<Plus size={18} />}>تسجيل غسيل جديد</Button> : undefined} />
      {loading ? <LoadingBlock /> : error ? <InlineRetry error={error} onRetry={load} /> : (
        <>
          <div className="metric-grid">
            <MetricCard label="السيارات اليوم" value={data.operational?.cars_today ?? 0} note="عملية مسجلة اليوم" icon={<Car size={21} />} />
            <MetricCard label="السيارات هذا الشهر" value={data.operational?.cars_this_month ?? 0} note="إجمالي عمليات الشهر" icon={<CalendarDays size={21} />} tone="teal" />
            {finance?.revenue_today !== undefined && <MetricCard label="إيراد اليوم" value={money(finance.revenue_today)} note={isManager ? 'إجمالي عمليات الزبائن والمعارض' : 'إجمالي عملياتك المكتملة اليوم'} icon={<CircleDollarSign size={21} />} tone="amber" />}
            {isManager && finance?.showroom_revenue_today !== undefined && <MetricCard label="إيراد المعارض اليوم" value={money(finance.showroom_revenue_today)} note="عمليات المعارض الآجلة فقط" icon={<Building2 size={21} />} tone="blue" />}
            {isManager && finance?.net_profit_today !== undefined && <MetricCard label="صافي ربح اليوم" value={money(finance.net_profit_today)} note="ربح عمليات الزبائن بعد عمولة العامل" icon={<Sparkles size={21} />} tone="teal" />}
            {isManager && finance?.showroom_net_profit_today !== undefined && <MetricCard label="صافي ربح المعارض اليوم" value={money(finance.showroom_net_profit_today)} note="ربح عمليات المعارض بعد عمولة العامل" icon={<ArrowUpLeft size={21} />} tone="violet" />}
          </div>
          {canFinancial && (
            <section className="dashboard-finance glass-card">
              <div className="dashboard-finance__header"><div><p className="eyebrow">ملخص الإدارة</p><h2>المؤشرات المالية لهذا الشهر</h2></div><button onClick={() => navigate('finance')}>التفاصيل المالية <ChevronLeft size={16} /></button></div>
              <div className="dashboard-finance__grid">
                <FinanceMini label="الإيراد الشهري" value={finance?.revenue_month} icon={<ArrowUpLeft size={17} />} />
                <FinanceMini label="حصة الأعمال" value={finance?.business_share_month} icon={<Banknote size={17} />} />
                <FinanceMini label="صافي الربح" value={finance?.net_profit_month} icon={<Sparkles size={17} />} accent />
                <FinanceMini label="مستحقات العمال" value={finance?.worker_payables} icon={<UsersRound size={17} />} />
                <FinanceMini label="مصاريف الشهر" value={finance?.expenses_month} icon={<ReceiptText size={17} />} />
                <FinanceMini label="ديون المعارض" value={finance?.showroom_outstanding} icon={<Building2 size={17} />} />
              </div>
            </section>
          )}
          <div className="dashboard-lower-grid">
            <SectionCard title="أحدث عمليات الغسيل" subtitle="آخر العمليات المسجلة في النظام" action={<button className="text-button" onClick={() => navigate('washes')}>عرض الكل <ChevronLeft size={15} /></button>}>
              {recent.length === 0 ? <EmptyState icon={<Car size={28} />} title="لا توجد عمليات بعد" description="ابدأ بإضافة أول عملية غسيل لهذا اليوم." action={<Button onClick={() => navigate('washes')} icon={<Plus size={17} />}>إضافة عملية</Button>} /> : <WashList washes={recent.slice(0, 5)} compact />}
            </SectionCard>
            <SectionCard title="تدفق العمل" subtitle="كل عملية تُسجّل مرة واحدة فقط">
              <div className="flow-steps">
                <FlowStep number="1" title="استقبال السيارة" text="بيانات السيارة والسعر" />
                <FlowStep number="2" title="تعيين العامل" text="يرتبط بسجل العامل" />
                <FlowStep number="3" title="الحفظ والتحديث" text="تُحدّث الحسابات تلقائيًا" />
              </div>
            </SectionCard>
          </div>
        </>
      )}
    </>
  );
}

function FinanceMini({ label, value, icon, accent = false }: { label: string; value: number | string | undefined; icon: ReactNode; accent?: boolean }) {
  return <div className={`finance-mini ${accent ? 'finance-mini--accent' : ''}`}><span>{icon}{label}</span><strong>{money(value)}</strong></div>;
}

function FlowStep({ number, title, text }: { number: string; title: string; text: string }) {
  return <div className="flow-step"><span>{number}</span><div><strong>{title}</strong><small>{text}</small></div></div>;
}

function InlineRetry({ error, onRetry }: { error: string; onRetry: () => void }) {
  return <div className="inline-retry glass-card"><AlertTriangle size={22} /><p>{error}</p><Button variant="secondary" onClick={onRetry} icon={<RefreshCw size={16} />}>إعادة المحاولة</Button></div>;
}

function WashList({ washes, compact = false, showWorkerEntitlement = true, showPaidStatus = false, showCreator = false, showCarColor = false, showWashType = false, paidUpdatingId, onTogglePaid, onEdit, onDelete, canDelete }: { washes: Wash[]; compact?: boolean; showWorkerEntitlement?: boolean; showPaidStatus?: boolean; showCreator?: boolean; showCarColor?: boolean; showWashType?: boolean; paidUpdatingId?: string | null; onTogglePaid?: (wash: Wash) => void | Promise<void>; onEdit?: (wash: Wash) => void; onDelete?: (wash: Wash) => void; canDelete?: (wash: Wash) => boolean }) {
  if (washes.length === 0) return <EmptyState title="لا توجد بيانات" />;
  return (
    <div className={`wash-list ${compact ? 'wash-list--compact' : ''} ${showWashType ? 'wash-list--with-type' : ''}`}>
      {washes.map((wash) => (
        <div className="wash-row" key={wash.id}>
          <div className="wash-row__car"><div><Car size={18} /></div><span><strong>{wash.vehicle_make} {wash.vehicle_model}</strong><small>{wash.license_plate || 'بدون لوحة'} {wash.manufacturing_year ? `• ${wash.manufacturing_year}` : ''}{showCarColor && wash.car_color ? ` • ${wash.car_color}` : ''}</small></span></div>
          {showWashType && <span className="wash-row__type"><small>نوع الغسيل</small><strong>{wash.wash_type || 'غير محدد'}</strong></span>}
          <span className="wash-row__worker"><UsersRound size={15} /><span>{wash.worker_name || 'عامل غير محدد'}{showCreator && <small>الحساب: {wash.created_by_name || 'غير محدد'}</small>}</span></span>
          {!compact && <span className="wash-row__date"><Clock3 size={15} />{dateFormat(wash.performed_at, true)}</span>}
          <span className="wash-row__statuses"><span className={wash.payment_type === 'cash' ? 'payment-pill payment-pill--cash' : 'payment-pill payment-pill--showroom'}>{wash.payment_type === 'cash' ? 'نقدي' : 'حساب معرض'}</span>{showPaidStatus && <span className={`payment-pill paid-status ${wash.is_paid ? 'is-paid' : ''}`}>{wash.is_paid ? 'خالصة' : 'غير خالصة'}</span>}</span>
          {wash.price !== undefined && <span className="wash-row__price"><strong>{money(wash.price)}</strong>{showWorkerEntitlement && wash.worker_due !== undefined && <small>مستحق العامل: {money(wash.worker_due)}</small>}</span>}
          {(onTogglePaid || onEdit || (onDelete && (!canDelete || canDelete(wash)))) && <div className="row-actions">{onTogglePaid && <button type="button" className={`paid-action ${wash.is_paid ? 'is-paid' : ''}`} disabled={paidUpdatingId === wash.id} aria-pressed={wash.is_paid === true} onClick={() => void onTogglePaid(wash)} aria-label={wash.is_paid ? 'إرجاع العملية إلى غير خالصة' : 'تعليم العملية كخالصة'} title={wash.is_paid ? 'إرجاع إلى غير خالصة' : 'تعليم كخالصة'}>{paidUpdatingId === wash.id ? <LoaderCircle className="spin" size={15} /> : wash.is_paid ? <Undo2 size={15} /> : <CheckCircle2 size={15} />}</button>}{onEdit && <button onClick={() => onEdit(wash)} aria-label="تعديل العملية"><Pencil size={15} /></button>}{onDelete && (!canDelete || canDelete(wash)) && <button className="danger-action" onClick={() => onDelete(wash)} aria-label="حذف العملية"><Trash2 size={15} /></button>}</div>}
        </div>
      ))}
    </div>
  );
}

function WashesView({ selectedDate, isManager, canWrite, onNotify }: { selectedDate: string; isManager: boolean; canWrite: boolean; onNotify: (message: ToastMessage) => void }) {
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [showrooms, setShowrooms] = useState<Showroom[]>([]);
  const [washes, setWashes] = useState<Wash[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [editing, setEditing] = useState<Wash | null>(null);
  const [paidUpdatingId, setPaidUpdatingId] = useState<string | null>(null);
  const cancellingWashIds = useRef(new Set<string>());
  const [form, setForm] = useState({ vehicle_make: '', vehicle_model: '', manufacturing_year: '', license_plate: '', car_color: '', wash_type: '', price: '', worker_id: '', performed_at: dateTimeInputValue(), payment_type: 'cash', showroom_id: '', showroom_payment_method: '' });

  const load = async () => {
    setLoading(true);
    try {
      const [loadedWorkers, loadedShowrooms, loadedWashes] = await Promise.all([
        api.workers({ status: 'active', include_financials: false }),
        api.showrooms({ include_financials: false }),
        api.washes({ date: selectedDate, limit: 300 }),
      ]);
      setWorkers(loadedWorkers);
      setShowrooms(loadedShowrooms);
      setWashes(loadedWashes);
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally { setLoading(false); }
  };

  useEffect(() => { void load(); }, [selectedDate]);
  const update = (key: keyof typeof form, value: string) => setForm((current) => ({ ...current, [key]: value }));

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (form.payment_type === 'showroom_account' && !form.showroom_id) {
      onNotify({ tone: 'error', text: 'يرجى اختيار المعرض عند استخدام حساب معرض.' });
      return;
    }
    if (form.payment_type === 'showroom_account' && !['cash', 'bank'].includes(form.showroom_payment_method)) {
      onNotify({ tone: 'error', text: 'يرجى اختيار طريقة دفع المعرض: نقدي أو مصرفي.' });
      return;
    }
    setSaving(true);
    try {
      await api.createWash({
        vehicle_make: form.vehicle_make.trim(),
        vehicle_model: form.vehicle_model.trim(),
        manufacturing_year: form.manufacturing_year ? Number(form.manufacturing_year) : null,
        license_plate: form.license_plate.trim() || null,
        car_color: form.car_color.trim() || null,
        wash_type: form.wash_type.trim() || null,
        price: form.price,
        worker_id: form.worker_id,
        performed_at: new Date(form.performed_at).toISOString(),
        payment_type: form.payment_type,
        showroom_id: form.payment_type === 'showroom_account' ? form.showroom_id : null,
        showroom_payment_method: form.payment_type === 'showroom_account' ? form.showroom_payment_method : null,
      });
      setForm({ vehicle_make: '', vehicle_model: '', manufacturing_year: '', license_plate: '', car_color: '', wash_type: '', price: '', worker_id: '', performed_at: dateTimeInputValue(), payment_type: 'cash', showroom_id: '', showroom_payment_method: '' });
      void load();
      refreshDashboard();
      onNotify({ tone: 'success', text: 'تم تسجيل عملية الغسيل وتحديث السجلات المرتبطة تلقائيًا.' });
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally { setSaving(false); }
  };
  const removeWash = async (wash: Wash) => {
    if (!window.confirm(`هل تريد حذف عملية ${wash.vehicle_make} ${wash.vehicle_model}؟ سيتم عكس آثارها المالية بأمان.`)) return;
    if (cancellingWashIds.current.has(wash.id)) return;
    cancellingWashIds.current.add(wash.id);
    try { await api.voidWash(wash.id, 'حذف العملية بواسطة مدير النظام'); setWashes((current) => current.filter((item) => item.id !== wash.id)); refreshDashboard(); onNotify({ tone: 'success', text: 'تم حذف العملية وعكس آثارها المالية.' }); }
    catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
    finally { cancellingWashIds.current.delete(wash.id); }
  };
  const togglePaid = async (wash: Wash) => {
    if (paidUpdatingId) return;
    setPaidUpdatingId(wash.id);
    try {
      const result = await api.setWashPaid(wash.id, wash.is_paid !== true, selectedDate);
      setWashes((current) => result.wash.is_paid
        ? current.filter((item) => item.id !== wash.id)
        : current.map((item) => item.id === wash.id ? result.wash : item));
      refreshDashboard();
      onNotify({ tone: 'success', text: result.wash.is_paid ? 'تم تعليم السيارة كخالصة وتحديث التسوية المالية.' : 'تم إرجاع السيارة إلى غير خالصة وتحديث التسوية المالية.' });
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally {
      setPaidUpdatingId(null);
    }
  };

  return (
    <>
      <PageHeader eyebrow="التشغيل اليومي" title="عمليات الغسيل" description={`عمليات يوم ${workingDateFormat(selectedDate)}. سجّل العملية مرة واحدة، ويتولى النظام ربطها بالسجلات ذات الصلة.`} />
      {canWrite && <div className="wash-page-grid">
        <SectionCard className="wash-form-card" title="تسجيل عملية غسيل جديدة" subtitle="الحقول المعلّمة مطلوبة لإتمام التسجيل.">
          {loading ? <LoadingBlock /> : (
            <form className="entry-form" onSubmit={submit}>
              <div className="form-grid form-grid--two">
                <FormField label="صانع المركبة" required><input value={form.vehicle_make} onChange={(event) => update('vehicle_make', event.target.value)} placeholder="مثال: تويوتا" required /></FormField>
                <FormField label="طراز المركبة" required><input value={form.vehicle_model} onChange={(event) => update('vehicle_model', event.target.value)} placeholder="مثال: كامري" required /></FormField>
                <FormField label="سنة الصنع"><input value={form.manufacturing_year} onChange={(event) => update('manufacturing_year', event.target.value.replace(/\D/g, '').slice(0, 4))} inputMode="numeric" placeholder="مثال: 2024" /></FormField>
                <FormField label="رقم اللوحة"><input value={form.license_plate} onChange={(event) => update('license_plate', event.target.value)} placeholder="اختياري" /></FormField>
                <FormField label="لون السيارة"><input value={form.car_color} onChange={(event) => update('car_color', event.target.value)} placeholder="اختياري" /></FormField>
                <FormField label="سعر الغسيل (د.ل)" required><input value={form.price} onChange={(event) => update('price', event.target.value)} type="number" min="0.01" step="0.01" placeholder="0.00" required dir="ltr" /></FormField>
                <FormField label="نوع الغسيل"><input value={form.wash_type} onChange={(event) => update('wash_type', event.target.value)} placeholder="مثال: غسيل كامل" /></FormField>
                <FormField label="العامل" required><select value={form.worker_id} onChange={(event) => update('worker_id', event.target.value)} required><option value="">اختر العامل</option>{workers.map((worker) => <option key={worker.id} value={worker.id}>{worker.name}</option>)}</select></FormField>
                <FormField label="التاريخ والوقت" required><input value={form.performed_at} onChange={(event) => update('performed_at', event.target.value)} type="datetime-local" required dir="ltr" /></FormField>
                <FormField label="نوع الزبون" required><select value={form.payment_type} onChange={(event) => setForm((current) => ({ ...current, payment_type: event.target.value, showroom_id: '', showroom_payment_method: '' }))}><option value="showroom_account">حساب معرض</option><option value="cash">زبون عادي</option></select></FormField>
              </div>
              {form.payment_type === 'showroom_account' && <div className="showroom-selection"><FormField label="المعرض" required><select value={form.showroom_id} onChange={(event) => setForm((current) => ({ ...current, showroom_id: event.target.value, showroom_payment_method: event.target.value ? (current.showroom_payment_method || 'cash') : '' }))} required><option value="">اختر المعرض</option>{showrooms.map((showroom) => <option key={showroom.id} value={showroom.id}>{showroom.name}</option>)}</select></FormField>{form.showroom_id && <FormField label="طريقة الدفع" required><select value={form.showroom_payment_method} onChange={(event) => update('showroom_payment_method', event.target.value)} required><option value="cash">نقدي</option><option value="bank">مصرفي</option></select></FormField>}<p><Building2 size={17} /> ستُضاف قيمة العملية تلقائيًا إلى حساب المعرض المحدد.</p></div>}
              <div className="form-actions"><p><Sparkles size={16} /> تُحسب العمولات وحصة الأعمال داخليًا بعد الحفظ.</p><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle className="spin" size={18} /> : <Save size={18} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ عملية الغسيل'}</Button></div>
            </form>
          )}
        </SectionCard>
        <aside className="wash-side-note glass-card"><div className="wash-side-note__icon"><ShieldCheck size={24} /></div><h3>سجل موحّد وآمن</h3><p>لا تحتاج إلى إدخال العملية مرة أخرى للعمال أو للمعارض؛ تُنشأ القيود المرتبطة تلقائيًا ضمن معاملة واحدة.</p><div><CheckCircle2 size={17} /> سجل العامل</div><div><CheckCircle2 size={17} /> حساب المعرض</div><div><CheckCircle2 size={17} /> التقارير</div></aside>
      </div>}
      <SectionCard title="أحدث العمليات" subtitle={`عمليات الغسيل المسجلة ليوم ${workingDateFormat(selectedDate)}.`} className="data-card">
        {loading ? <LoadingBlock /> : washes.length ? <WashList washes={washes} showWashType showPaidStatus paidUpdatingId={paidUpdatingId} onTogglePaid={canWrite ? togglePaid : undefined} onEdit={canWrite ? setEditing : undefined} onDelete={canWrite ? removeWash : undefined} /> : <EmptyState icon={<Car size={28} />} title="لا توجد عمليات مسجلة حتى الآن" description="استخدم النموذج أعلاه لإضافة أول عملية." />}
      </SectionCard>
      {editing && <EditWashModal wash={editing} workers={workers} showrooms={showrooms} isManager={isManager || canWrite} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); void load(); refreshDashboard(); onNotify({ tone: 'success', text: 'تم تعديل العملية وإعادة احتساب جميع آثارها.' }); }} onNotify={onNotify} />}
    </>
  );
}

function PaidCarsView({ selectedDate, isManager, canWrite, onNotify }: { selectedDate: string; isManager: boolean; canWrite: boolean; onNotify: (message: ToastMessage) => void }) {
  const [items, setItems] = useState<Wash[]>([]);
  const [settlement, setSettlement] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [paidUpdatingId, setPaidUpdatingId] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await api.paidCars({ date: selectedDate });
      setItems(result.items);
      setSettlement(safeNumber(result.settlement));
    } catch (requestError) {
      setError(friendlyError(requestError));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, [selectedDate]);

  const revertToUnpaid = async (wash: Wash) => {
    if (paidUpdatingId) return;
    if (!window.confirm('هل تريد إرجاع هذه السيارة إلى أحدث العمليات؟')) return;
    setPaidUpdatingId(wash.id);
    try {
      const result = await api.setWashPaid(wash.id, false, selectedDate);
      setItems((current) => current.filter((item) => item.id !== wash.id));
      setSettlement(result.settlement);
      refreshFinancialViews();
      onNotify({ tone: 'success', text: 'تم إرجاع السيارة إلى غير خالصة وإعادة احتساب التسوية المالية.' });
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    } finally {
      setPaidUpdatingId(null);
    }
  };

  return <>
    <PageHeader eyebrow="سجل التشغيل" title="السيارات الخالصة" description={`${isManager ? 'جميع العمليات الخالصة' : 'العمليات الخالصة المرتبطة بحسابك'} ليوم ${workingDateFormat(selectedDate)}.`} actions={<Button variant="secondary" onClick={() => void load()} icon={<RefreshCw size={17} />}>تحديث</Button>} />
    <div className="metric-grid paid-cars-summary">
      <MetricCard label="عدد السيارات الخالصة" value={items.length} note={isManager ? 'من جميع حسابات الموظفين' : 'من عمليات حسابك فقط'} icon={<CheckCircle2 size={21} />} tone="teal" />
      <MetricCard label="التسوية المالية" value={money(settlement)} note="إجمالي أسعار السيارات الخالصة فقط" icon={<CircleDollarSign size={21} />} tone="blue" />
    </div>
    <SectionCard title="سجل السيارات الخالصة" subtitle={items.length ? `${items.length} عملية خالصة مرتبطة بسجل الغسيل الأصلي.` : 'تظهر هنا العمليات بعد اعتماد علامة الخالص.'} className="data-card">
      {loading ? <LoadingBlock /> : error ? <InlineRetry error={error} onRetry={load} /> : items.length ? <WashList washes={items} showWashType showPaidStatus showCreator={isManager} showCarColor showWorkerEntitlement={isManager} paidUpdatingId={paidUpdatingId} onTogglePaid={canWrite ? revertToUnpaid : undefined} /> : <EmptyState icon={<CheckCircle2 size={29} />} title="لا توجد سيارات خالصة" description="استخدم علامة الصح بجانب عملية الغسيل لاعتمادها كخالصة." />}
    </SectionCard>
  </>;
}

function EditWashModal({ wash, workers, showrooms, isManager, onClose, onSaved, onNotify }: { wash: Wash; workers: Worker[]; showrooms: Showroom[]; isManager: boolean; onClose: () => void; onSaved: () => void; onNotify: (message: ToastMessage) => void }) {
  const [form,setForm]=useState({vehicle_make:wash.vehicle_make,vehicle_model:wash.vehicle_model,manufacturing_year:wash.manufacturing_year?.toString()??'',license_plate:wash.license_plate??'',car_color:wash.car_color??'',wash_type:wash.wash_type??'',price:String(wash.price??''),worker_id:wash.worker_id??'',performed_at:dateTimeInputValue(new Date(wash.performed_at)),payment_type:wash.payment_type,showroom_id:wash.showroom_id??'',showroom_payment_method:wash.showroom_payment_method??(wash.payment_type==='showroom_account'?'cash':'')});
  const [markAsOvernight,setMarkAsOvernight]=useState(wash.is_overnight===true);
  const [saving,setSaving]=useState(false); const update=(key:keyof typeof form,value:string)=>setForm(current=>({...current,[key]:value}));
  const submit=async(event:FormEvent)=>{event.preventDefault();if(form.payment_type==='showroom_account'&&(!form.showroom_id||!['cash','bank'].includes(form.showroom_payment_method))){onNotify({tone:'error',text:'اختر المعرض وطريقة الدفع.'});return;}setSaving(true);try{await api.updateWash(wash.id,{...form,manufacturing_year:form.manufacturing_year?Number(form.manufacturing_year):null,license_plate:form.license_plate.trim()||null,car_color:form.car_color.trim()||null,wash_type:form.wash_type.trim()||null,performed_at:new Date(form.performed_at).toISOString(),showroom_id:form.payment_type==='showroom_account'?form.showroom_id:null,showroom_payment_method:form.payment_type==='showroom_account'?form.showroom_payment_method:null,mark_as_overnight:markAsOvernight});onSaved();}catch(error){onNotify({tone:'error',text:friendlyError(error)});}finally{setSaving(false);}};
  return <Modal title="تعديل عملية الغسيل" subtitle="يعيد النظام احتساب العمولة وحصة المركز وحساب المعرض تلقائيًا." onClose={onClose} wide><form className="entry-form" onSubmit={submit}><div className="form-grid form-grid--two"><FormField label="صانع المركبة" required><input value={form.vehicle_make} onChange={e=>update('vehicle_make',e.target.value)} required /></FormField><FormField label="الطراز" required><input value={form.vehicle_model} onChange={e=>update('vehicle_model',e.target.value)} required /></FormField><FormField label="سنة الصنع"><input value={form.manufacturing_year} onChange={e=>update('manufacturing_year',e.target.value)} type="number" /></FormField><FormField label="رقم اللوحة"><input value={form.license_plate} onChange={e=>update('license_plate',e.target.value)} /></FormField><FormField label="لون السيارة"><input value={form.car_color} onChange={e=>update('car_color',e.target.value)} /></FormField><FormField label="السعر" required><input value={form.price} onChange={e=>update('price',e.target.value)} type="number" min="0.01" step="0.01" required dir="ltr" /></FormField><FormField label="نوع الغسيل"><input value={form.wash_type} onChange={e=>update('wash_type',e.target.value)} placeholder="مثال: غسيل كامل" /></FormField><FormField label="العامل" required><select value={form.worker_id} onChange={e=>update('worker_id',e.target.value)} required>{workers.map(worker=><option key={worker.id} value={worker.id}>{worker.name}</option>)}</select></FormField><FormField label="التاريخ والوقت" required><input value={form.performed_at} onChange={e=>update('performed_at',e.target.value)} type="datetime-local" required /></FormField><FormField label="نوع الزبون"><select value={form.payment_type} onChange={e=>setForm(current=>({...current,payment_type:e.target.value as Wash['payment_type'],showroom_id:'',showroom_payment_method:''}))}><option value="showroom_account">حساب معرض</option><option value="cash">زبون عادي</option></select></FormField></div>{form.payment_type==='showroom_account'&&<div className="showroom-selection"><FormField label="المعرض" required><select value={form.showroom_id} onChange={e=>setForm(current=>({...current,showroom_id:e.target.value,showroom_payment_method:e.target.value?(current.showroom_payment_method||'cash'):''}))} required><option value="">اختر المعرض</option>{showrooms.map(showroom=><option key={showroom.id} value={showroom.id}>{showroom.name}</option>)}</select></FormField>{form.showroom_id&&<FormField label="طريقة الدفع" required><select value={form.showroom_payment_method} onChange={e=>update('showroom_payment_method',e.target.value)}><option value="cash">نقدي</option><option value="bank">مصرفي</option></select></FormField>}</div>}{isManager&&<div className="permission-toggle-row"><div><strong>Mark as Overnight Car / تعليم كسيارة مبيتة</strong><span>{markAsOvernight?'ستظهر السيارة في سجل سيارات المبيت.':'لن تظهر السيارة في سجل سيارات المبيت.'}</span></div><button type="button" className={`permission-switch ${markAsOvernight?'is-on':''}`} role="switch" aria-checked={markAsOvernight} onClick={()=>setMarkAsOvernight(current=>!current)}><span>{markAsOvernight?'مفعّل':'متوقف'}</span><i aria-hidden="true" /></button></div>}<div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={<Save size={17}/>}>{saving?'جارٍ الحفظ...':'حفظ التعديل'}</Button></div></form></Modal>;
}

function OvernightCarsView({ selectedDate, canWrite, onNotify }: { selectedDate: string; canWrite: boolean; onNotify: (message: ToastMessage) => void }) {
  const [items,setItems]=useState<OvernightCar[]>([]);
  const [workers,setWorkers]=useState<Worker[]>([]);
  const [showrooms,setShowrooms]=useState<Showroom[]>([]);
  const [editing,setEditing]=useState<OvernightCar|null>(null);
  const [deletingId,setDeletingId]=useState<string|null>(null);
  const [loading,setLoading]=useState(true);
  const [error,setError]=useState<string|null>(null);
  const load=async()=>{setLoading(true);setError(null);try{const [overnight,activeWorkers,availableShowrooms]=await Promise.all([api.overnightCars({date:selectedDate}),api.workers({status:'active',include_financials:false}),api.showrooms({include_financials:false})]);setItems(overnight);setWorkers(activeWorkers);setShowrooms(availableShowrooms);}catch(requestError){setError(friendlyError(requestError));}finally{setLoading(false);}};
  useEffect(()=>{void load();},[selectedDate]);
  const remove=async(item:OvernightCar)=>{if(deletingId||!window.confirm(`هل تريد حذف سجل سيارة ${item.wash.vehicle_make} ${item.wash.vehicle_model}؟ ستبقى عملية الغسيل الأصلية محفوظة.`))return;setDeletingId(item.id);try{await api.deleteOvernightCar(item.id);setItems(current=>current.filter(entry=>entry.id!==item.id));onNotify({tone:'success',text:'تم حذف سجل سيارة المبيت مع الاحتفاظ بعملية الغسيل الأصلية.'});}catch(requestError){onNotify({tone:'error',text:friendlyError(requestError)});}finally{setDeletingId(null);}};
  return <><PageHeader eyebrow="سجل التشغيل" title="سيارات المبيت" description={`السيارات المرتبطة بعمليات يوم ${workingDateFormat(selectedDate)} والمعلّمة للمبيت.`}/><SectionCard title="سجل سيارات المبيت" subtitle={items.length?`${items.length} سيارة مسجلة للمبيت`:'تظهر هنا السيارات التي يحددها المستخدم من تعديل عملية الغسيل.'} className="data-card">{loading?<LoadingBlock/>:error?<InlineRetry error={error} onRetry={load}/>:items.length?<WashList washes={items.map(item=>item.wash)} showWashType onEdit={canWrite ? (wash=>{const item=items.find(entry=>entry.wash.id===wash.id);if(item)setEditing(item);}) : undefined} onDelete={canWrite ? (wash=>{const item=items.find(entry=>entry.wash.id===wash.id);if(item)void remove(item);}) : undefined}/>:<EmptyState icon={<Moon size={28}/>} title="لا توجد سيارات مبيت" description="يمكن تعليم السيارة للمبيت من نافذة تعديل عملية الغسيل."/>}</SectionCard>{editing&&<EditWashModal wash={editing.wash} workers={workers} showrooms={showrooms} isManager={canWrite} onClose={()=>setEditing(null)} onSaved={()=>{setEditing(null);void load();onNotify({tone:'success',text:'تم تحديث بيانات سيارة المبيت والعملية المرتبطة.'});}} onNotify={onNotify}/>}</>;
}

function WorkersView({ selectedDate, currentUserId, isManager, canWrite, canFinancial, onNotify }: { selectedDate: string; currentUserId: string; isManager: boolean; canWrite: boolean; canFinancial: boolean; onNotify: (message: ToastMessage) => void }) {
  const range = selectedDateRange(selectedDate);
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      const result = await api.workers({ date: selectedDate, include_financials: canFinancial });
      setWorkers(result);
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally { setLoading(false); }
  };

  useEffect(() => { void load(); }, [selectedDate]);
  const filteredWorkers = workers.filter((worker) => worker.name.toLowerCase().includes(query.toLowerCase()));

  return (
    <>
      <PageHeader eyebrow="سجل فريق العمل" title="العمال" description="متابعة النشاط والمستحقات ضمن ملف موحد لكل عامل." actions={canWrite ? <Button variant="secondary" onClick={() => setAddOpen(true)} icon={<UserPlus size={18} />}>إضافة عامل</Button> : undefined} />
      <SectionCard className="filter-card">
        <div className="filter-bar">
          <div className="search-box"><Search size={18} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="ابحث باسم العامل" /></div>
          <div className="date-filter"><CalendarDays size={17} /><span>تاريخ العمل:</span><strong>{workingDateFormat(selectedDate)}</strong></div>
        </div>
      </SectionCard>
      {loading ? <LoadingBlock /> : filteredWorkers.length === 0 ? <EmptyState icon={<UsersRound size={29} />} title="لا يوجد عمال مطابقون" description="أضف عاملًا جديدًا أو غيّر معايير البحث." /> : (
        <div className="worker-grid">
          {filteredWorkers.map((worker) => <WorkerCard key={worker.id} worker={worker} canFinancial={canFinancial} onClick={() => setSelectedId(worker.id)} />)}
        </div>
      )}
      {selectedId && <WorkerProfile workerId={selectedId} selectedDate={selectedDate} currentUserId={currentUserId} isManager={isManager} canWrite={canWrite} canFinancial={canFinancial} range={range} onClose={() => setSelectedId(null)} onChanged={() => { setSelectedId(null); void load(); }} onDeleted={(deletedWorkerId) => { setSelectedId(null); setWorkers((current) => current.filter((worker) => worker.id !== deletedWorkerId)); void load(); }} onNotify={onNotify} />}
      {canWrite && addOpen && <CreateWorkerModal onClose={() => setAddOpen(false)} onSaved={(worker) => { setWorkers((current) => [worker, ...current]); setAddOpen(false); onNotify({ tone: 'success', text: 'تمت إضافة العامل إلى فريق العمل.' }); }} onNotify={onNotify} />}
    </>
  );
}

function WorkerCard({ worker, canFinancial, onClick }: { worker: Worker; canFinancial: boolean; onClick: () => void }) {
  const count = worker.cars_washed ?? worker.washes_count ?? 0;
  const finance = worker.financials;
  return (
    <button className="worker-card glass-card" onClick={onClick}>
      <div className="worker-card__header"><div className="worker-avatar">{worker.name.slice(0, 1)}</div><div><h3>{worker.name}</h3><span>{worker.phone || 'بدون رقم اتصال'}</span></div><ChevronLeft size={18} /></div>
      <div className="worker-card__stats"><span><Car size={16} />{count} سيارة</span><span><Clock3 size={16} />{worker.latest_wash_at ? dateFormat(worker.latest_wash_at) : 'لا توجد عملية حديثة'}</span></div>
      {canFinancial && finance && <div className="worker-card__finance"><span>الرصيد المستحق</span><strong>{money(finance.payable_balance)}</strong></div>}
      {!canFinancial && <div className="worker-card__operational-note"><Activity size={16} /> عرض النشاط التشغيلي</div>}
    </button>
  );
}

function WorkerProfile({ workerId, selectedDate, currentUserId, isManager, canWrite, canFinancial, range, onClose, onChanged, onDeleted, onNotify }: { workerId: string; selectedDate: string; currentUserId: string; isManager: boolean; canWrite: boolean; canFinancial: boolean; range: DateRange; onClose: () => void; onChanged: () => void; onDeleted: (workerId: string) => void; onNotify: (message: ToastMessage) => void }) {
  const [worker, setWorker] = useState<(Worker & { washes?: Wash[] }) | null>(null);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState(false);
  const [ledgerOpen, setLedgerOpen] = useState(false);
  const deletingWashIds = useRef(new Set<string>());

  useEffect(() => {
    let mounted = true;
    const load = async () => {
      setLoading(true);
      try {
        const result = await api.worker(workerId, canFinancial, selectedDate);
        if (mounted) setWorker(result);
      } catch (error) {
        onNotify({ tone: 'error', text: friendlyError(error) });
        onClose();
      } finally { if (mounted) setLoading(false); }
    };
    void load();
    return () => { mounted = false; };
  }, [workerId, selectedDate, canFinancial]);

  const removeWash = async (wash: Wash) => {
    if (deletingWashIds.current.has(wash.id) || !window.confirm(`هل تريد حذف عملية ${wash.vehicle_make} ${wash.vehicle_model} من سجل العامل؟ سيتم عكس آثارها المالية بأمان.`)) return;
    deletingWashIds.current.add(wash.id);
    try {
      await api.voidWash(wash.id, 'حذف عملية الغسيل من ملف العامل');
      refreshDashboard();
      const refreshed = await api.worker(workerId, canFinancial, selectedDate);
      setWorker(refreshed);
      onNotify({ tone: 'success', text: 'تم حذف عملية الغسيل وتحديث إجماليات العامل.' });
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally { deletingWashIds.current.delete(wash.id); }
  };

  if (!worker && loading) return <SidePanel title="ملف العامل" onClose={onClose}><LoadingBlock /></SidePanel>;
  if (!worker) return null;
  const financials = worker.financials;
  return (
    <SidePanel title="ملف العامل" onClose={onClose}>
      <div className="profile-heading"><div className="profile-heading__avatar">{worker.name.slice(0, 1)}</div><div><h2>{worker.name}</h2><span>{worker.phone || 'لا يوجد رقم اتصال'}</span></div></div>
      {canWrite && <div className="profile-actions"><Button variant="secondary" onClick={() => setEditing(true)} icon={<Pencil size={16}/>}>تعديل العامل</Button>{isManager && <Button variant="danger" onClick={async()=>{if(!window.confirm(`هل تريد حذف العامل المحدد «${worker.name}»؟ سيتم حذفه من قائمة العمال مع الاحتفاظ بعمليات الغسيل والسجلات التاريخية المرتبطة به.`))return;try{await api.deleteWorker(worker.id);onNotify({tone:'success',text:`تم حذف العامل «${worker.name}» من قائمة العمال مع الاحتفاظ بسجله التاريخي.`});onDeleted(worker.id);}catch(error){onNotify({tone:'error',text:friendlyError(error)});}}} icon={<Trash2 size={16}/>}>حذف العامل</Button>}</div>}
      <div className="profile-period"><CalendarDays size={16} /> الفترة المعروضة: {dateFormat(range.from)} — {dateFormat(range.to)}</div>
      <div className="profile-operational-metrics"><MetricCard label="السيارات المغسولة" value={worker.cars_washed ?? worker.washes_count ?? 0} icon={<Car size={18} />} /><MetricCard label="إجمالي قيمة الغسيل" value={money(worker.total_wash_value)} icon={<Banknote size={18} />} tone="teal" /></div>
      {financials && (
        <section className="profile-financials">
          <div className="sensitive-label"><LockKeyhole size={14} /> معلومات مالية مخولة</div>
          <div className="mini-metric-grid profile-commission-summary">
            <MiniMetric label="إجمالي العمولة" value={financials.gross_commission} />
          </div>
        </section>
      )}
      {isManager && <button type="button" className="worker-ledger-entry glass-card" onClick={() => setLedgerOpen(true)}><div className="worker-ledger-entry__icon"><WalletCards size={21} /></div><div><strong>المسحوبات والمرتجعات</strong><span>السجل الكامل والرصيد القائم للعامل</span></div><ChevronLeft size={18} /></button>}
      <section className="profile-history"><div className="profile-section-heading"><h3>سجل عمليات الغسيل</h3><span>{worker.washes?.length ?? 0} عملية</span></div>{worker.washes?.length ? <WashList washes={worker.washes} compact onDelete={canWrite ? (wash) => void removeWash(wash) : undefined} canDelete={(wash) => isManager || wash.created_by_id === currentUserId} /> : <EmptyState title="لا توجد عمليات ضمن هذه الفترة" />}</section>
      {editing&&<EditWorkerModal worker={worker} canFinancial={canFinancial} onClose={()=>setEditing(false)} onSaved={()=>{setEditing(false);onChanged();onNotify({tone:'success',text:'تم تحديث بيانات العامل.'});}} onNotify={onNotify}/>}
      {ledgerOpen && <WorkerWithdrawalsReturnsView workerId={worker.id} workerName={worker.name} selectedDate={selectedDate} onClose={() => setLedgerOpen(false)} onNotify={onNotify} />}
    </SidePanel>
  );
}

function WorkerWithdrawalsReturnsView({ workerId, workerName, selectedDate, onClose, onNotify }: { workerId: string; workerName: string; selectedDate: string; onClose: () => void; onNotify: (message: ToastMessage) => void }) {
  const [ledger, setLedger] = useState<WorkerWithdrawalReturnLedger | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [entryType, setEntryType] = useState<'withdrawal' | 'return' | 'deduction_payment' | null>(null);
  const [editingPayment, setEditingPayment] = useState<WorkerWithdrawalReturnLedger['transactions'][number] | null>(null);
  const [settling, setSettling] = useState(false);
  const [deletingMovementId, setDeletingMovementId] = useState<string | null>(null);
  const load = async () => { setLoading(true); setError(null); try { setLedger(await api.workerWithdrawalReturns(workerId, { from: '0000-01-01T00:00:00Z', to: '9999-12-31T23:59:59Z' })); } catch (requestError) { setError(friendlyError(requestError)); } finally { setLoading(false); } };
  useEffect(() => { void load(); }, [workerId, selectedDate]);
  const settle = async () => {
    if (!ledger || Number(ledger.outstanding_balance) <= 0 || !window.confirm(`هل تريد تصفية كامل الرصيد القائم للعامل ${workerName}؟ ستُحفظ التصفية كحركة مستقلة ولن تُحذف الحركات السابقة.`)) return;
    setSettling(true);
    try {
      await api.settleWorkerWithdrawalReturns(workerId, new Date(`${selectedDate}T12:00:00`).toISOString());
      await load();
      onNotify({ tone: 'success', text: 'تمت تصفية المستقطعات وحفظ حركة التصفية في السجل.' });
    } catch (requestError) { onNotify({ tone: 'error', text: friendlyError(requestError) }); }
    finally { setSettling(false); }
  };
  const removeMovement = async (movementId: string, movementLabel: string) => {
    if (deletingMovementId || !window.confirm(`هل تريد حذف حركة ${movementLabel}؟ ستُعاد جميع الإجماليات من السجلات المتبقية.`)) return;
    setDeletingMovementId(movementId);
    try {
      await api.deleteWorkerWithdrawalReturn(workerId, movementId);
      await load();
      onNotify({ tone: 'success', text: 'تم حذف الحركة وإعادة احتساب الرصيد من السجل المحفوظ.' });
    } catch (requestError) { onNotify({ tone: 'error', text: friendlyError(requestError) }); }
    finally { setDeletingMovementId(null); }
  };
  return <Modal title={`المسحوبات والمرتجعات — ${workerName}`} subtitle="السجل الكامل للعامل؛ الحركات مستقلة ولا تؤثر على عمولات عمليات الغسيل." onClose={onClose} wide>
    {loading ? <LoadingBlock /> : error ? <InlineRetry error={error} onRetry={() => void load()} /> : ledger && <>
      <div className="mini-metric-grid worker-ledger-summary"><MiniMetric label="إجمالي المسحوبات" value={ledger.total_withdrawals} tone="danger" /><MiniMetric label="إجمالي الاستقطاعات" value={ledger.total_deductions} tone="danger" /><MiniMetric label="إجمالي المرتجعات" value={ledger.total_returns} tone="success" /><MiniMetric label="تسديدات الاستقطاع" value={ledger.total_deduction_payments} tone="success" /><MiniMetric label="إجمالي التسويات" value={ledger.total_settlements} /><MiniMetric label="الرصيد القائم" value={ledger.outstanding_balance} tone="warning" /></div>
      <div className="profile-actions"><Button onClick={() => setEntryType('withdrawal')} icon={<ArrowDownLeft size={17} />}>إضافة مسحوب</Button><Button variant="secondary" onClick={() => setEntryType('return')} icon={<ArrowUpLeft size={17} />}>إضافة مرتجع</Button><Button variant="secondary" onClick={() => setEntryType('deduction_payment')} disabled={Number(ledger.outstanding_balance) <= 0} icon={<Banknote size={17} />}>تسديد استقطاع</Button><Button variant="secondary" onClick={() => void settle()} disabled={settling || Number(ledger.outstanding_balance) <= 0} icon={settling ? <LoaderCircle size={17} className="spin" /> : <CheckCircle2 size={17} />}>{settling ? 'جارٍ التصفية...' : 'تصفية المستقطعات'}</Button></div>
      <SectionCard title="سجل الحركات" subtitle={`${ledger.transactions.length} حركة مسجلة للعامل المحدد فقط.`}>
        {ledger.transactions.length === 0 ? <EmptyState icon={<WalletCards size={27} />} title="لا توجد حركات مالية" /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>النوع</th><th>المبلغ</th><th>التاريخ</th><th>الملاحظة</th><th>سجله</th><th>إجراء</th></tr></thead><tbody>{ledger.transactions.map((transaction) => {
          const label = transaction.type === 'withdrawal' ? 'مسحوب' : transaction.type === 'deduction' ? 'استقطاع' : transaction.type === 'return' ? 'مرتجع' : transaction.type === 'deduction_payment' ? 'تسديد استقطاع' : 'تصفية';
          const tone = transaction.type === 'withdrawal' || transaction.type === 'deduction' ? 'danger' : transaction.type === 'return' || transaction.type === 'deduction_payment' ? 'success' : 'info';
          return <tr key={transaction.id}><td><StatusBadge tone={tone}>{label}</StatusBadge></td><td className="money-cell">{money(transaction.amount)}</td><td>{dateFormat(transaction.occurred_at)}</td><td>{transaction.notes || '—'}</td><td>{transaction.created_by_name || '—'}</td><td><div className="worker-ledger-row-actions">{transaction.editable && <button type="button" className="table-action worker-ledger-edit" onClick={() => setEditingPayment(transaction)} title="تعديل تسديد الاستقطاع" aria-label="تعديل تسديد الاستقطاع"><Pencil size={15} /></button>}{transaction.deletable ? <button type="button" className="table-action danger-action worker-ledger-delete" onClick={() => void removeMovement(transaction.id, label)} disabled={deletingMovementId === transaction.id} title={`حذف حركة ${label}`} aria-label={`حذف حركة ${label}`}>{deletingMovementId === transaction.id ? <LoaderCircle size={15} className="spin" /> : <Trash2 size={15} />}</button> : <span className="table-muted" title="يُدار الاستقطاع من المصروف المرتبط">—</span>}</div></td></tr>;
        })}</tbody></table></div>}
      </SectionCard>
    </>}
    {entryType && <WorkerWithdrawalReturnModal type={entryType} workerId={workerId} selectedDate={selectedDate} onClose={() => setEntryType(null)} onSaved={() => { setEntryType(null); void load(); onNotify({ tone: 'success', text: entryType === 'withdrawal' ? 'تم تسجيل المسحوب وتحديث الرصيد القائم.' : entryType === 'return' ? 'تم تسجيل المرتجع وتحديث الرصيد القائم.' : 'تم تسجيل تسديد الاستقطاع وتحديث الرصيد القائم.' }); }} onNotify={onNotify} />}
    {editingPayment && <WorkerWithdrawalReturnModal type="deduction_payment" workerId={workerId} selectedDate={selectedDate} movement={editingPayment} onClose={() => setEditingPayment(null)} onSaved={() => { setEditingPayment(null); void load(); onNotify({ tone: 'success', text: 'تم تعديل تسديد الاستقطاع وإعادة احتساب الرصيد.' }); }} onNotify={onNotify} />}
  </Modal>;
}

function WorkerWithdrawalReturnModal({ type, workerId, selectedDate, movement, onClose, onSaved, onNotify }: { type: 'withdrawal' | 'return' | 'deduction_payment'; workerId: string; selectedDate: string; movement?: WorkerWithdrawalReturnLedger['transactions'][number]; onClose: () => void; onSaved: () => void; onNotify: (message: ToastMessage) => void }) {
  const [amount, setAmount] = useState(movement ? String(movement.amount) : '');
  const [date, setDate] = useState(movement?.occurred_at.slice(0, 10) || selectedDate);
  const [notes, setNotes] = useState(movement?.notes || '');
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => { event.preventDefault(); setSaving(true); try { const data = { amount, occurred_at: new Date(`${date}T12:00:00`).toISOString(), notes: notes.trim() || null }; if (movement) await api.updateWorkerDeductionPayment(workerId, movement.id, data); else await api.createWorkerWithdrawalReturn(workerId, { type, ...data }); onSaved(); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); } };
  const title = movement ? 'تعديل تسديد استقطاع' : type === 'withdrawal' ? 'إضافة مسحوب' : type === 'return' ? 'إضافة مرتجع' : 'تسديد استقطاع';
  return <Modal title={title} onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="المبلغ" required><input value={amount} onChange={(event) => setAmount(event.target.value)} type="number" min="0.001" step="0.001" required dir="ltr" /></FormField><FormField label="التاريخ" required><input value={date} onChange={(event) => setDate(event.target.value)} type="date" required /></FormField><FormField label="ملاحظة"><textarea value={notes} onChange={(event) => setNotes(event.target.value)} /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : movement ? 'حفظ التعديل' : 'حفظ الحركة'}</Button></div></form></Modal>;
}

function EditWorkerModal({worker,canFinancial,onClose,onSaved,onNotify}:{worker:Worker;canFinancial:boolean;onClose:()=>void;onSaved:()=>void;onNotify:(message:ToastMessage)=>void}){
  const [form,setForm]=useState({name:worker.name,phone:worker.phone??'',notes:worker.notes??'',status:worker.status??'active',commission_percentage:String(worker.financials?.commission_percentage??'')});const[saving,setSaving]=useState(false);
  const submit=async(event:FormEvent)=>{event.preventDefault();setSaving(true);try{await api.updateWorker(worker.id,{...form,phone:form.phone.trim()||null,notes:form.notes.trim()||null,commission_percentage:form.commission_percentage||null});onSaved();}catch(error){onNotify({tone:'error',text:friendlyError(error)});}finally{setSaving(false);}};
  return <Modal title="تعديل بيانات العامل" onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="اسم العامل" required><input value={form.name} onChange={e=>setForm(c=>({...c,name:e.target.value}))} required/></FormField><FormField label="رقم الاتصال"><input value={form.phone} onChange={e=>setForm(c=>({...c,phone:e.target.value}))}/></FormField><FormField label="ملاحظات"><textarea value={form.notes} onChange={e=>setForm(c=>({...c,notes:e.target.value}))}/></FormField><div className="form-grid form-grid--two"><FormField label="الحالة"><select value={form.status} onChange={e=>setForm(c=>({...c,status:e.target.value}))}><option value="active">نشط</option><option value="inactive">غير نشط</option></select></FormField>{canFinancial&&<FormField label="نسبة العمولة الخاصة (%)" hint="اتركها فارغة لاستخدام النسبة الافتراضية."><input value={form.commission_percentage} onChange={e=>setForm(c=>({...c,commission_percentage:e.target.value}))} type="number" min="0" max="100" step="0.01" dir="ltr" /></FormField>}</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={<Save size={17}/>}>{saving?'جارٍ الحفظ...':'حفظ التعديل'}</Button></div></form></Modal>;
}

function MiniMetric({ label, value, tone = 'neutral' }: { label: string; value: number | string | undefined; tone?: 'neutral' | 'danger' | 'success' | 'warning' }) {
  return <div className={`mini-metric mini-metric--${tone}`}><span>{label}</span><strong>{money(value)}</strong></div>;
}

function CreateWorkerModal({ onClose, onSaved, onNotify }: { onClose: () => void; onSaved: (worker: Worker) => void; onNotify: (message: ToastMessage) => void }) {
  const [name, setName] = useState('');
  const [phone, setPhone] = useState('');
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try { onSaved(await api.createWorker({ name: name.trim(), phone: phone.trim() || null })); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); }
  };
  return <Modal title="إضافة عامل جديد" subtitle="أضف البيانات الأساسية للعامل." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="اسم العامل" required><input value={name} onChange={(event) => setName(event.target.value)} required placeholder="الاسم الكامل" /></FormField><FormField label="رقم الاتصال"><input value={phone} onChange={(event) => setPhone(event.target.value)} placeholder="اختياري" dir="ltr" /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ العامل'}</Button></div></form></Modal>;
}

function ShowroomDebtsView({ selectedDate, reportIssuer, onNotify }: { selectedDate: string; reportIssuer: string; onNotify: (message: ToastMessage) => void }) {
  const [items, setItems] = useState<ShowroomDebtSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const load = async () => {
    setLoading(true);
    try { setItems(await api.showroomDebts({ date: selectedDate })); }
    catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
    finally { setLoading(false); }
  };
  useEffect(() => {
    void load();
    const handleRefresh = () => { void load(); };
    window.addEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
    return () => window.removeEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
  }, [selectedDate]);
  const filtered = items.filter((item) => item.showroom.name.toLowerCase().includes(query.toLowerCase()));
  return <>
    <PageHeader eyebrow="الحسابات المالية" title="ديون المعارض" description={`أرصدة المعارض كما كانت حتى نهاية ${workingDateFormat(selectedDate)}، مستمدة مباشرة من العمليات والدفعات المسجلة.`} actions={<Button variant="secondary" onClick={() => void load()} icon={<RefreshCw size={17} />}>تحديث</Button>} />
    <SectionCard className="filter-card"><div className="filter-bar"><div className="search-box"><Search size={18} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="ابحث باسم المعرض" /></div><span className="filter-hint"><Building2 size={17} /> تظهر المعارض التي لديها عمليات غسيل آجلة قائمة فقط.</span></div></SectionCard>
    {loading ? <LoadingBlock label="جارٍ تحميل ديون المعارض..." /> : filtered.length === 0 ? <EmptyState icon={<Building2 size={29} />} title="لا توجد ديون معارض قائمة" description="ستظهر هنا تلقائيًا أي عملية غسيل جديدة مسجلة على حساب معرض." /> : <div className="showroom-grid">{filtered.map((item) => <ShowroomDebtCard key={item.showroom.id} item={item} onClick={() => setSelectedId(item.showroom.id)} />)}</div>}
    {selectedId && <ShowroomDebtProfileView showroomId={selectedId} selectedDate={selectedDate} reportIssuer={reportIssuer} onClose={() => setSelectedId(null)} onNotify={onNotify} />}
  </>;
}

function ShowroomDebtCard({ item, onClick }: { item: ShowroomDebtSummary; onClick: () => void }) {
  return <article className="showroom-card glass-card" onClick={onClick} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') onClick(); }} role="button" tabIndex={0}>
    <div className="showroom-card__top"><div className="showroom-icon"><Building2 size={21} /></div><div><h3>{item.showroom.name}</h3><span>{item.showroom.contact_name || item.showroom.phone || 'معرض شريك'}</span></div><ChevronLeft size={18} /></div>
    <div className="showroom-card__activity"><Car size={16} /><span>{formatNumber(item.outstanding_wash_count)} سيارة على الحساب</span>{item.latest_wash_at && <><i /><Clock3 size={15} /><span>{dateFormat(item.latest_wash_at)}</span></>}</div>
    <div className="showroom-card__balance"><span>إجمالي الدين القائم</span><strong>{money(item.total_outstanding)}</strong></div>
  </article>;
}

function ShowroomDebtProfileView({ showroomId, selectedDate, reportIssuer, onClose, onNotify }: { showroomId: string; selectedDate: string; reportIssuer: string; onClose: () => void; onNotify: (message: ToastMessage) => void }) {
  const [range, setRange] = useState<DateRange>(() => selectedDateRange(selectedDate));
  const [profile, setProfile] = useState<ShowroomDebtProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [editingPayment, setEditingPayment] = useState<PaymentRecord | null>(null);
  const [reportGeneratedAt, setReportGeneratedAt] = useState(() => new Date());
  const rangeIsComplete = Boolean(range.from && range.to);
  const rangeIsValid = rangeIsComplete && range.from <= range.to;
  const printDebtReport = () => {
    setReportGeneratedAt(new Date());
    window.requestAnimationFrame(() => window.print());
  };
  useEffect(() => { setRange(selectedDateRange(selectedDate)); }, [selectedDate]);
  useEffect(() => {
    if (!rangeIsValid) { setLoading(false); return; }
    let mounted = true;
    setLoading(true);
    setError(null);
    api.showroomDebt(showroomId, range)
      .then((result) => { if (mounted) setProfile(result); })
      .catch((requestError) => { if (mounted) { setError(friendlyError(requestError)); onNotify({ tone: 'error', text: friendlyError(requestError) }); } })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; };
  }, [showroomId, range.from, range.to, rangeIsValid, refreshKey]);
  useEffect(() => {
    const handleRefresh = () => setRefreshKey((current) => current + 1);
    window.addEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
    return () => window.removeEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
  }, []);
  if (!profile && loading) return <Modal title="ملف ديون المعرض" onClose={onClose} wide><LoadingBlock label="جارٍ تحميل عمليات الدين..." /></Modal>;
  if (!profile && error) return <Modal title="ملف ديون المعرض" onClose={onClose} wide><InlineRetry error={error} onRetry={() => setRefreshKey((current) => current + 1)} /></Modal>;
  if (!profile) return null;
  return <Modal title={`ديون معرض ${profile.showroom.name}`} subtitle="العمليات المعروضة مرتبطة مباشرة بسجل عمليات الغسيل الأصلي." onClose={onClose} wide>
    <div className="showroom-debt-profile">
      <div className="profile-heading"><div className="profile-heading__avatar profile-heading__avatar--showroom"><Building2 size={26} /></div><div><h2>{profile.showroom.name}</h2><span>{profile.showroom.contact_name || 'معرض شريك'}</span></div></div>
      <div className="mini-metric-grid showroom-debt-summary"><div className="mini-metric"><span>عدد السيارات القائمة</span><strong>{formatNumber(profile.outstanding_wash_count)} سيارة</strong></div><MiniMetric label="إجمالي قيمة الغسيل" value={profile.total_charges} /><MiniMetric label="إجمالي الدفعات" value={profile.total_payments} tone="success" /><MiniMetric label="الرصيد المتبقي" value={profile.total_outstanding} tone="warning" /></div>
      <SectionCard title="فترة تقرير الدين" subtitle="القائمة والإجماليات والتقرير المطبوع تعتمد على تاريخ عملية الغسيل الفعلي." action={<div className="showroom-debt-print-controls">
        <label><span>من تاريخ</span><input type="date" value={range.from} onChange={(event) => setRange((current) => ({ ...current, from: event.target.value }))} /></label>
        <label><span>إلى تاريخ</span><input type="date" value={range.to} onChange={(event) => setRange((current) => ({ ...current, to: event.target.value }))} /></label>
        <Button onClick={printDebtReport} disabled={loading || !rangeIsValid} icon={loading ? <LoaderCircle size={17} className="spin" /> : <Printer size={17} />}>طباعة تقرير الدين</Button>
      </div>}>
        {!rangeIsComplete ? <div className="inline-alert inline-alert--error"><AlertTriangle size={17} /> يرجى تحديد تاريخ البداية وتاريخ النهاية.</div> : range.from > range.to ? <div className="inline-alert inline-alert--error"><AlertTriangle size={17} /> تاريخ البداية يجب أن يسبق تاريخ النهاية.</div> : <div className="date-filter"><CalendarDays size={17} /><span>الفترة المختارة:</span><strong>{workingDateFormat(range.from)} — {workingDateFormat(range.to)}</strong></div>}
      </SectionCard>
      <SectionCard title="عمليات الغسيل الآجلة" subtitle={`${formatNumber(profile.outstanding_wash_count)} عملية ضمن الفترة المحددة.`}>
        {loading ? <LoadingBlock label="جارٍ تحديث الفترة..." /> : profile.operations.length === 0 ? <EmptyState icon={<Car size={28} />} title="لا توجد عمليات دين ضمن الفترة المحددة" /> : <div className="data-table-wrap"><table className="data-table showroom-debt-table"><thead><tr><th>السيارة</th><th>اللوحة</th><th>اللون</th><th>العامل</th><th>طريقة السداد</th><th>التاريخ والوقت</th><th>السعر</th></tr></thead><tbody>{profile.operations.map((operation) => <tr key={operation.id}><td><strong>{operation.vehicle_make} {operation.vehicle_model}</strong><small>{operation.manufacturing_year || '—'}</small></td><td>{operation.license_plate || '—'}</td><td>{operation.car_color || '—'}</td><td>{operation.worker_name || '—'}</td><td>{operation.showroom_payment_method === 'bank' ? 'مصرفي' : 'نقدي'}</td><td>{dateFormat(operation.performed_at, true)}</td><td className="money-cell">{money(operation.price)}</td></tr>)}</tbody></table></div>}
      </SectionCard>
      <SectionCard title="سجل دفعات المعرض" subtitle={`${profile.payments.length} دفعة ضمن الفترة المحددة.`}>
        {profile.payments.length === 0 ? <EmptyState icon={<Banknote size={28} />} title="لا توجد دفعات ضمن الفترة المحددة" /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>المبلغ</th><th>التاريخ</th><th>ملاحظات</th><th>سجله</th><th>إجراءات</th></tr></thead><tbody>{profile.payments.map((payment) => <tr key={payment.id}><td className="money-cell">{money(payment.amount)}</td><td>{dateFormat(payment.paid_at || payment.date, true)}</td><td>{payment.notes || '—'}</td><td>{payment.created_by_name || '—'}</td><td><div className="row-actions"><button type="button" onClick={() => setEditingPayment(payment)} aria-label="تعديل الدفعة" title="تعديل الدفعة"><Pencil size={15} /></button><button type="button" className="danger-action" onClick={async () => { if (!window.confirm('هل تريد حذف هذه الدفعة؟ سيعود المبلغ إلى رصيد المعرض.')) return; try { await api.deleteShowroomPayment(payment.id); refreshFinancialViews(); onNotify({ tone: 'success', text: 'تم حذف الدفعة وإعادة احتساب رصيد المعرض.' }); } catch (requestError) { onNotify({ tone: 'error', text: friendlyError(requestError) }); } }} aria-label="حذف الدفعة" title="حذف الدفعة"><Trash2 size={15} /></button></div></td></tr>)}</tbody></table></div>}
      </SectionCard>
    </div>
    <ShowroomDebtPrintReport profile={profile} range={range} reportIssuer={reportIssuer} generatedAt={reportGeneratedAt} />
    {editingPayment && <ShowroomPaymentModal payment={editingPayment} showrooms={[profile.showroom]} onClose={() => setEditingPayment(null)} onNotify={onNotify} onSaved={() => { setEditingPayment(null); refreshFinancialViews(); onNotify({ tone: 'success', text: 'تم تعديل الدفعة وإعادة احتساب رصيد المعرض.' }); }} />}
  </Modal>;
}

function ShowroomDebtPrintReport({ profile, range, reportIssuer, generatedAt }: { profile: ShowroomDebtProfile; range: DateRange; reportIssuer: string; generatedAt: Date }) {
  let unappliedPayments = Math.max(0, safeNumber(profile.total_payments));
  const paidByOperation = new Map<string, number>();
  [...profile.operations]
    .sort((first, second) => new Date(first.performed_at).getTime() - new Date(second.performed_at).getTime())
    .forEach((operation) => {
      const price = Math.max(0, safeNumber(operation.price));
      const paid = Math.min(price, unappliedPayments);
      paidByOperation.set(operation.id, paid);
      unappliedPayments = Math.max(0, unappliedPayments - paid);
    });
  const reportRows = profile.operations.map((operation) => {
    const invoice = Math.max(0, safeNumber(operation.price));
    const paid = paidByOperation.get(operation.id) ?? 0;
    const remaining = Math.max(0, invoice - paid);
    return { operation, invoice, paid, remaining, status: remaining <= .0001 ? 'مسدد' : paid > 0 ? 'مدفوع جزئياً' : 'مستحق' };
  });
  const agingBuckets = [
    { label: 'حتى 30 يوماً', color: '#61ad4d', amount: 0 },
    { label: '31 - 60 يوماً', color: '#ff9b17', amount: 0 },
    { label: '61 - 90 يوماً', color: '#ef5350', amount: 0 },
    { label: 'أكثر من 90 يوماً', color: '#c81d25', amount: 0 },
  ];
  reportRows.forEach(({ operation, remaining }) => {
    const washDate = new Date(operation.performed_at);
    const ageDays = Number.isNaN(washDate.getTime()) ? 0 : Math.max(0, Math.floor((generatedAt.getTime() - washDate.getTime()) / 86_400_000));
    const bucketIndex = ageDays <= 30 ? 0 : ageDays <= 60 ? 1 : ageDays <= 90 ? 2 : 3;
    agingBuckets[bucketIndex].amount += remaining;
  });
  const agingTotal = agingBuckets.reduce((total, bucket) => total + bucket.amount, 0);
  let agingCursor = 0;
  const agingStops = agingBuckets.map((bucket) => {
    const start = agingCursor;
    agingCursor += agingTotal > 0 ? (bucket.amount / agingTotal) * 100 : 0;
    return `${bucket.color} ${start}% ${agingCursor}%`;
  });
  const agingChart = agingTotal > 0 ? `conic-gradient(${agingStops.join(', ')})` : 'conic-gradient(#e7edf5 0 100%)';
  const accountStart = profile.showroom.created_at || [...profile.operations]
    .sort((first, second) => new Date(first.performed_at).getTime() - new Date(second.performed_at).getTime())[0]?.performed_at;

  return <section className="showroom-debt-print" dir="rtl">
    <header className="showroom-debt-print__header">
      <div className="showroom-debt-print__mark"><span>صدر بواسطة</span><strong>{reportIssuer}</strong></div>
      <div className="showroom-debt-print__title"><h1>تقرير دين المعرض</h1><p><CalendarDays size={15} /> تاريخ إصدار التقرير: <strong>{workingDateFormat(businessDateKey(generatedAt))}</strong></p></div>
      <div className="showroom-debt-print__business"><strong>{DEBT_REPORT_BUSINESS_NAME}</strong></div>
    </header>

    <section className="showroom-debt-print__showroom">
      <h2>بيانات المعرض</h2>
      <div className="showroom-debt-print__info-grid">
        <div className="showroom-debt-print__info"><span><Building2 size={17} /> اسم المعرض</span><strong>{profile.showroom.name}</strong></div>
        <div className="showroom-debt-print__info"><span><Phone size={17} /> رقم الهاتف</span><strong dir="ltr">{profile.showroom.phone || '—'}</strong></div>
        <div className="showroom-debt-print__info"><span><MapPin size={17} /> العنوان</span><strong>{profile.showroom.address || '—'}</strong></div>
        <div className="showroom-debt-print__info"><span><CalendarDays size={17} /> تاريخ بداية الحساب</span><strong>{dateFormat(accountStart)}</strong></div>
      </div>
    </section>

    <section className="showroom-debt-print__summary" aria-label="ملخص الدين">
      <div className="showroom-debt-print__summary-card showroom-debt-print__summary-card--cars"><span>عدد السيارات</span><strong>{formatNumber(profile.outstanding_wash_count)}</strong><Car size={23} /></div>
      <div className="showroom-debt-print__summary-card showroom-debt-print__summary-card--remaining"><span>إجمالي المتبقي</span><strong>{money(profile.total_outstanding)}</strong><FileText size={23} /></div>
      <div className="showroom-debt-print__summary-card showroom-debt-print__summary-card--paid"><span>إجمالي المدفوع</span><strong>{money(profile.total_payments)}</strong><Banknote size={23} /></div>
      <div className="showroom-debt-print__summary-card showroom-debt-print__summary-card--debt"><span>إجمالي الدين</span><strong>{money(profile.total_charges)}</strong><WalletCards size={23} /></div>
    </section>

    <section className="showroom-debt-print__details">
      <h2><FileText size={17} /> تفاصيل الديون</h2>
      <table>
        <thead><tr><th>م</th><th>نوع السيارة</th><th>تاريخ الغسيل</th><th>إجمالي الفاتورة (د.ل)</th><th>المدفوع</th><th>المتبقي (د.ل)</th><th>الحالة</th></tr></thead>
        <tbody>{reportRows.map(({ operation, invoice, paid, remaining, status }, index) => <tr key={operation.id}>
          <td>{formatNumber(index + 1)}</td>
          <td><strong>{operation.vehicle_make} {operation.vehicle_model}</strong><small>{[operation.license_plate, operation.car_color].filter(Boolean).join(' · ') || '—'}</small></td>
          <td>{dateFormat(operation.performed_at)}</td>
          <td>{money(invoice, '')}</td>
          <td>{money(paid, '')}</td>
          <td>{money(remaining, '')}</td>
          <td><span className={`showroom-debt-print__status showroom-debt-print__status--${status === 'مسدد' ? 'paid' : status === 'مدفوع جزئياً' ? 'partial' : 'due'}`}>{status}</span></td>
        </tr>)}</tbody>
        <tfoot><tr><th colSpan={3}>الإجمالي</th><th>{money(profile.total_charges, '')}</th><th>{money(profile.total_payments, '')}</th><th>{money(profile.total_outstanding, '')}</th><th>د.ل</th></tr></tfoot>
      </table>
    </section>

    <section className="showroom-debt-print__bottom">
      <div className="showroom-debt-print__notes"><h2>ملاحظات</h2><p>يغطي هذا التقرير الفترة من {dateFormat(range.from)} إلى {dateFormat(range.to)}.</p></div>
      <div className="showroom-debt-print__aging"><h2>ملخص أعمار الدين</h2><div className="showroom-debt-print__aging-content"><div className="showroom-debt-print__donut" style={{ background: agingChart }}><i /></div><div className="showroom-debt-print__aging-list">{agingBuckets.map((bucket) => <div key={bucket.label}><i style={{ background: bucket.color }} /><span>{bucket.label}</span><strong>{money(bucket.amount)}</strong></div>)}</div></div><div className="showroom-debt-print__balance"><span>إجمالي المتبقي</span><strong>{money(profile.total_outstanding)}</strong></div></div>
    </section>
  </section>;
}

function ShowroomsView({ selectedDate, canFinancial, canWrite, onNotify }: { selectedDate: string; canFinancial: boolean; canWrite: boolean; onNotify: (message: ToastMessage) => void }) {
  const [showrooms, setShowrooms] = useState<Showroom[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [editingShowroom, setEditingShowroom] = useState<Showroom | null>(null);

  const load = async () => {
    setLoading(true);
    try { setShowrooms(await api.showrooms({ date: selectedDate, include_financials: canFinancial })); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, [canFinancial, selectedDate]);
  const filteredShowrooms = showrooms.filter((showroom) => showroom.name.toLowerCase().includes(query.toLowerCase()));

  return (
    <>
      <PageHeader eyebrow="حسابات الشركاء" title="المعارض" description={canFinancial ? 'متابعة المعارض وعملياتها والأرصدة المستحقة بشكل آمن.' : 'عرض نشاط المعارض وعمليات المركبات المسجلة.'} actions={canWrite ? <Button variant="secondary" onClick={() => setAddOpen(true)} icon={<Plus size={18} />}>إضافة معرض</Button> : undefined} />
      <SectionCard className="filter-card"><div className="filter-bar"><div className="search-box"><Search size={18} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="ابحث باسم المعرض" /></div><span className="filter-hint"><Building2 size={17} /> {canFinancial ? 'تظهر الأرصدة لحسابك.' : 'بيانات تشغيلية دون تفاصيل مالية.'}</span></div></SectionCard>
      {loading ? <LoadingBlock /> : filteredShowrooms.length === 0 ? <EmptyState icon={<Building2 size={29} />} title="لا توجد معارض مطابقة" description="أضف معرضًا لتسجيل عملياته على الحساب." /> : <div className="showroom-grid">{filteredShowrooms.map((showroom) => <ShowroomCard key={showroom.id} showroom={showroom} isManager={canFinancial} canWrite={canWrite} onClick={() => setSelectedId(showroom.id)} onEdit={() => setEditingShowroom(showroom)} onDelete={async () => { if (!window.confirm('هل تريد حذف هذا المعرض نهائيًا؟ لا يمكن التراجع عن هذا الإجراء.')) return; try { await api.deleteShowroom(showroom.id); setShowrooms((current) => current.filter((item) => item.id !== showroom.id)); if (selectedId === showroom.id) setSelectedId(null); onNotify({ tone: 'success', text: 'تم حذف المعرض بنجاح.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } }} />)}</div>}
      {selectedId && <ShowroomProfile showroomId={selectedId} selectedDate={selectedDate} isManager={canFinancial} onClose={() => setSelectedId(null)} onNotify={onNotify} />}
      {canWrite && addOpen && <CreateShowroomModal onClose={() => setAddOpen(false)} onSaved={(showroom) => { setShowrooms((current) => [showroom, ...current]); setAddOpen(false); onNotify({ tone: 'success', text: 'تمت إضافة المعرض بنجاح.' }); }} onNotify={onNotify} />}
      {canWrite && editingShowroom && <CreateShowroomModal showroom={editingShowroom} onClose={() => setEditingShowroom(null)} onSaved={(showroom) => { setShowrooms((current) => current.map((item) => item.id === showroom.id ? { ...item, ...showroom } : item)); setEditingShowroom(null); onNotify({ tone: 'success', text: 'تم تحديث بيانات المعرض.' }); }} onNotify={onNotify} />}
    </>
  );
}

function ShowroomCard({ showroom, isManager, canWrite, onClick, onEdit, onDelete }: { showroom: Showroom; isManager: boolean; canWrite: boolean; onClick: () => void; onEdit: () => void; onDelete: () => void }) {
  const financials = showroom.financials;
  return (
    <article className="showroom-card glass-card" onClick={onClick} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') onClick(); }} role="button" tabIndex={0}>
      <div className="showroom-card__top"><div className="showroom-icon"><Building2 size={21} /></div><div><h3>{showroom.name}</h3><span>{showroom.contact_name || showroom.phone || 'جهة شريكة'}</span></div><ChevronLeft size={18} /></div>
      <div className="showroom-card__activity"><Car size={16} /><span>{showroom.washes_count ?? 0} عملية غسيل</span>{showroom.latest_wash_at && <><i /> <Clock3 size={15} /><span>{dateFormat(showroom.latest_wash_at)}</span></>}</div>
      {isManager && financials ? <div className="showroom-card__balance"><span>الرصيد المستحق</span><strong>{money(financials.outstanding_balance)}</strong></div> : <div className="showroom-card__operational"><Activity size={16} /> استعراض نشاط المعرض</div>}
      {canWrite && <div className="showroom-card__actions"><button type="button" className="icon-button" onClick={(event) => { event.stopPropagation(); onEdit(); }} aria-label="تعديل المعرض" title="تعديل المعرض"><Pencil size={17} /></button><button type="button" className="icon-button danger-action" onClick={(event) => { event.stopPropagation(); onDelete(); }} aria-label="حذف المعرض" title="حذف المعرض"><Trash2 size={17} /></button></div>}
    </article>
  );
}

function ShowroomProfile({ showroomId, selectedDate, isManager, onClose, onNotify }: { showroomId: string; selectedDate: string; isManager: boolean; onClose: () => void; onNotify: (message: ToastMessage) => void }) {
  const [showroom, setShowroom] = useState<(Showroom & { washes?: Wash[] }) | null>(null);
  const [loading, setLoading] = useState(true);
  const [statisticsLoading, setStatisticsLoading] = useState(true);
  const [carCount, setCarCount] = useState(0);
  const [statisticsRange, setStatisticsRange] = useState<DateRange>(() => selectedDateRange(selectedDate));
  const [paymentType, setPaymentType] = useState<'all' | 'cash' | 'debt'>('all');
  useEffect(() => {
    let mounted = true;
    const load = async () => {
      try { const result = await api.showroom(showroomId, isManager, selectedDate); if (mounted) setShowroom(result); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); onClose(); } finally { if (mounted) setLoading(false); }
    };
    void load();
    const handleRefresh = () => { void load(); };
    window.addEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
    return () => { mounted = false; window.removeEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh); };
  }, [showroomId, isManager, selectedDate]);
  useEffect(() => { setStatisticsRange(selectedDateRange(selectedDate)); }, [selectedDate]);
  useEffect(() => {
    if (!statisticsRange.from || !statisticsRange.to || statisticsRange.from > statisticsRange.to) return;
    let mounted = true;
    setStatisticsLoading(true);
    api.showroomStatistics(showroomId, { ...statisticsRange, paymentType })
      .then((count) => { if (mounted) setCarCount(count); })
      .catch((error) => { if (mounted) onNotify({ tone: 'error', text: friendlyError(error) }); })
      .finally(() => { if (mounted) setStatisticsLoading(false); });
    return () => { mounted = false; };
  }, [showroomId, statisticsRange.from, statisticsRange.to, paymentType]);
  if (!showroom && loading) return <SidePanel title="ملف المعرض" onClose={onClose}><LoadingBlock /></SidePanel>;
  if (!showroom) return null;
  const finance = isManager ? showroom.financials : undefined;
  return <SidePanel title="ملف المعرض" onClose={onClose}>
    <div className="profile-heading"><div className="profile-heading__avatar profile-heading__avatar--showroom"><Building2 size={26} /></div><div><h2>{showroom.name}</h2><span>{showroom.contact_name || 'معرض شريك'}</span></div></div>
    <div className="showroom-details"><span><Phone size={16} /> {showroom.phone || 'لا يوجد رقم اتصال'}</span>{showroom.address && <span>الموقع: {showroom.address}</span>}</div>
    <div className="exhibition-profile-card-stack">
      {isManager && finance && <section className="profile-financials"><div className="sensitive-label"><LockKeyhole size={14} /> تفاصيل مالية مخولة</div><div className="mini-metric-grid"><MiniMetric label="إجمالي الرسوم" value={finance.total_charges} /><MiniMetric label="المدفوعات" value={finance.total_payments} tone="success" /><MiniMetric label="الرصيد المستحق" value={finance.outstanding_balance} tone="warning" /></div></section>}
      {isManager && <SectionCard title="سجل دفعات المعرض" subtitle="دفعات منفصلة مرتبطة بهذا المعرض فقط.">{showroom.payments?.length ? <div className="data-table-wrap"><table className="data-table"><thead><tr><th>المبلغ</th><th>التاريخ</th><th>ملاحظات</th><th>سجله</th></tr></thead><tbody>{showroom.payments.map((payment) => <tr key={payment.id}><td className="money-cell">{money(payment.amount)}</td><td>{dateFormat(payment.paid_at || payment.date, true)}</td><td>{payment.notes || '—'}</td><td>{payment.created_by_name || '—'}</td></tr>)}</tbody></table></div> : <EmptyState icon={<Banknote size={26} />} title="لا توجد دفعات مسجلة لهذا المعرض" />}</SectionCard>}
      <SectionCard title="إحصائيات السيارات" subtitle="العدد الفعلي للعمليات حسب التاريخ وطريقة الدفع.">
      <div className="showroom-statistics-filters">
        <FormField label="من تاريخ"><input type="date" value={statisticsRange.from} onChange={(event) => setStatisticsRange((current) => ({ ...current, from: event.target.value }))} /></FormField>
        <FormField label="إلى تاريخ"><input type="date" value={statisticsRange.to} onChange={(event) => setStatisticsRange((current) => ({ ...current, to: event.target.value }))} /></FormField>
        <FormField label="طريقة الدفع"><select value={paymentType} onChange={(event) => setPaymentType(event.target.value as typeof paymentType)}><option value="all">الكل</option><option value="cash">نقدي</option><option value="debt">دين</option></select></FormField>
      </div>
      {statisticsRange.from > statisticsRange.to ? <div className="inline-alert inline-alert--error"><AlertTriangle size={17} /> تاريخ البداية يجب أن يسبق تاريخ النهاية.</div> : <div className="showroom-statistics-result"><Car size={20} /><span>عدد السيارات</span><strong>{statisticsLoading ? '...' : formatNumber(carCount)}</strong></div>}
      </SectionCard>
      <section className="profile-history"><div className="profile-section-heading"><h3>سجل الغسيل</h3><span>{showroom.washes?.length ?? 0} عملية</span></div>{showroom.washes?.length ? <WashList washes={showroom.washes} compact /> : <EmptyState title="لا توجد عمليات مسجلة" />}</section>
    </div>
  </SidePanel>;
}

function CreateShowroomModal({ showroom, onClose, onSaved, onNotify }: { showroom?: Showroom; onClose: () => void; onSaved: (showroom: Showroom) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ name: showroom?.name ?? '', contact_name: showroom?.contact_name ?? '', phone: showroom?.phone ?? '', address: showroom?.address ?? '' });
  const [saving, setSaving] = useState(false);
  const update = (key: keyof typeof form, value: string) => setForm((current) => ({ ...current, [key]: value }));
  const submit = async (event: FormEvent) => {
    event.preventDefault(); setSaving(true);
    try { const data = { name: form.name.trim(), contact_name: form.contact_name.trim() || null, phone: form.phone.trim() || null, address: form.address.trim() || null }; onSaved(showroom ? await api.updateShowroom(showroom.id, data) : await api.createShowroom(data)); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); }
  };
  return <Modal title={showroom ? 'تعديل بيانات المعرض' : 'إضافة معرض جديد'} subtitle="أدخل بيانات الجهة التي ترسل المركبات للحساب." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="اسم المعرض" required><input value={form.name} onChange={(event) => update('name', event.target.value)} placeholder="اسم المعرض أو الوكالة" required /></FormField><FormField label="اسم جهة الاتصال"><input value={form.contact_name} onChange={(event) => update('contact_name', event.target.value)} placeholder="اختياري" /></FormField><FormField label="رقم الاتصال"><input value={form.phone} onChange={(event) => update('phone', event.target.value)} placeholder="اختياري" dir="ltr" /></FormField><FormField label="العنوان"><input value={form.address} onChange={(event) => update('address', event.target.value)} placeholder="اختياري" /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : showroom ? 'حفظ التعديلات' : 'حفظ المعرض'}</Button></div></form></Modal>;
}

function SidePanel({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  return <div className="side-panel-layer"><div className="side-panel-layer__backdrop" onClick={onClose} /><aside className="side-panel"><header><div><p className="eyebrow">تفاصيل السجل</p><h2>{title}</h2></div><button className="icon-button" onClick={onClose} aria-label="إغلاق"><X size={21} /></button></header><div className="side-panel__content">{children}</div></aside></div>;
}

function Modal({ title, subtitle, children, onClose, wide = false }: { title: string; subtitle?: string; children: ReactNode; onClose: () => void; wide?: boolean }) {
  return <div className="modal-layer" role="dialog" aria-modal="true" aria-label={title}><div className="modal-layer__backdrop" onClick={onClose} /><section className={`modal glass-card ${wide ? 'modal--wide' : ''}`}><header><div><h2>{title}</h2>{subtitle && <p>{subtitle}</p>}</div><button className="icon-button" onClick={onClose} aria-label="إغلاق"><X size={21} /></button></header>{children}</section></div>;
}

function SalariesView({ selectedDate, onNotify }: { selectedDate: string; onNotify: (message: ToastMessage) => void }) {
  const month = selectedDate.slice(0, 7);
  const [summary, setSummary] = useState<PayrollSummary | null>(null);
  const [withdrawals, setWithdrawals] = useState<SalaryWithdrawal[]>([]);
  const [deductions, setDeductions] = useState<SalaryDeduction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [addEmployeeOpen, setAddEmployeeOpen] = useState(false);
  const [salaryEmployee, setSalaryEmployee] = useState<PayrollEmployee | null>(null);
  const [withdrawalEmployee, setWithdrawalEmployee] = useState<PayrollEmployee | null>(null);
  const [deductionEmployee, setDeductionEmployee] = useState<PayrollEmployee | null>(null);
  const [editingWithdrawal, setEditingWithdrawal] = useState<SalaryWithdrawal | null>(null);
  const [editingDeduction, setEditingDeduction] = useState<SalaryDeduction | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextSummary, nextWithdrawals, nextDeductions] = await Promise.all([
        api.payroll(month, selectedDate),
        api.salaryWithdrawals(month, selectedDate),
        api.salaryDeductions(month, selectedDate),
      ]);
      setSummary(nextSummary);
      setWithdrawals(nextWithdrawals);
      setDeductions(nextDeductions);
    } catch (requestError) {
      setError(friendlyError(requestError));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, [month, selectedDate]);

  const removeWithdrawal = async (withdrawal: SalaryWithdrawal) => {
    if (!window.confirm(`هل تريد حذف مسحوب الموظف «${withdrawal.employee_name}» بقيمة ${money(withdrawal.amount)}؟`)) return;
    try {
      await api.deleteSalaryWithdrawal(withdrawal.id);
      await load();
      onNotify({ tone: 'success', text: 'تم حذف المسحوب وإعادة احتساب الراتب المتبقي.' });
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    }
  };

  const removeDeduction = async (deduction: SalaryDeduction) => {
    if (!window.confirm(`هل تريد حذف خصم الموظف «${deduction.employee_name}» بقيمة ${money(deduction.amount)}؟`)) return;
    try {
      await api.deleteSalaryDeduction(deduction.id);
      await load();
      onNotify({ tone: 'success', text: 'تم حذف الخصم وإعادة احتساب الراتب المتبقي.' });
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    }
  };

  const removeEmployee = async (employee: PayrollEmployee) => {
    if (!window.confirm(`هل تريد حذف الموظف «${employee.employee_name}» من قسم المرتبات؟ سيتم الاحتفاظ بكل سجلات المرتبات التاريخية.`)) return;
    try {
      await api.deletePayrollEmployee(employee.employee_id);
      await load();
      onNotify({ tone: 'success', text: 'تمت أرشفة الموظف من المرتبات مع الاحتفاظ بسجله التاريخي.' });
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    }
  };

  const employees = summary?.employees ?? [];
  return (
    <>
      <PageHeader
        eyebrow="وصول الإدارة فقط"
        title="المرتبات"
        description="إدارة المرتبات الشهرية ومسحوبات الموظفين مع سجل مستقل لكل شهر."
        actions={<><Button variant="secondary" onClick={() => setAddEmployeeOpen(true)} icon={<UserPlus size={17} />}>إضافة موظف</Button><Button onClick={() => setWithdrawalEmployee(employees[0] ?? null)} disabled={employees.length === 0} icon={<Plus size={17} />}>تسجيل مسحوب</Button></>}
      />
      <SectionCard className="filter-card">
        <div className="filter-bar payroll-filter">
          <div className="date-filter"><CalendarDays size={17} /><span>شهر تاريخ العمل</span><strong>{month}</strong></div>
          <span className="filter-hint"><History size={16} /> يتحدد شهر المرتب من تاريخ العمل العالمي، وتظل حركات اليوم أدناه مقيدة بيوم {workingDateFormat(selectedDate)}.</span>
          <Button variant="secondary" onClick={() => void load()} icon={<RefreshCw size={17} />}>تحديث</Button>
        </div>
      </SectionCard>
      {loading ? <LoadingBlock label="جارٍ تحميل المرتبات والمسحوبات..." /> : error ? <InlineRetry error={error} onRetry={() => void load()} /> : summary && (
        <>
          <div className="metric-grid payroll-metrics">
            <MetricCard label="إجمالي المرتبات" value={money(summary.total_salary)} note={`شهر ${summary.month}`} icon={<Banknote size={21} />} tone="blue" />
            <MetricCard label="إجمالي المسحوبات" value={money(summary.total_withdrawals)} note="المسحوبات المسجلة خلال الشهر" icon={<ArrowDownLeft size={21} />} tone="amber" />
            <MetricCard label="إجمالي المتبقي" value={money(summary.total_remaining)} note="المرتب ناقص المسحوبات والخصومات" icon={<CircleDollarSign size={21} />} tone="teal" />
          </div>
          <SectionCard title="مرتبات الموظفين" subtitle="المرتب المحدد للشهر يستمر للأشهر التالية حتى تسجيل قيمة جديدة.">
            {employees.length === 0 ? <EmptyState icon={<UsersRound size={28} />} title="لا يوجد موظفون مسجلون" /> : <div className="data-table-wrap"><table className="data-table payroll-table"><thead><tr><th>الموظف</th><th>المرتب</th><th>إجمالي المسحوبات</th><th>إجمالي الخصومات</th><th>المتبقي</th><th>إجراءات</th></tr></thead><tbody>{employees.map((employee) => <tr key={employee.employee_id}><td><strong>{employee.employee_name}</strong></td><td className="money-cell">{employee.salary_configured ? money(employee.salary) : <StatusBadge tone="warning">غير محدد</StatusBadge>}</td><td className="money-cell">{money(employee.total_withdrawals)}</td><td className="money-cell">{money(employee.total_deductions)}</td><td className={`money-cell ${safeNumber(employee.remaining_salary) < 0 ? 'salary-remaining--negative' : ''}`}>{money(employee.remaining_salary)}</td><td><div className="row-actions"><button onClick={() => setSalaryEmployee(employee)} aria-label={`تعديل مرتب ${employee.employee_name}`} title="تعديل المرتب"><Pencil size={15} /></button><button onClick={() => setWithdrawalEmployee(employee)} aria-label={`مسحوبات الموظف ${employee.employee_name}`} title="مسحوبات"><Plus size={15} /></button><button onClick={() => setDeductionEmployee(employee)} aria-label={`خصم للموظف ${employee.employee_name}`} title="خصم"><Minus size={15} /></button><button className="danger-action" onClick={() => void removeEmployee(employee)} aria-label={`حذف الموظف ${employee.employee_name}`} title="حذف الموظف"><Trash2 size={15} /></button></div></td></tr>)}</tbody></table></div>}
          </SectionCard>
          <SectionCard title="مسحوبات الموظف" subtitle={`سجل المسحوبات ليوم ${workingDateFormat(selectedDate)}.`} action={<Button onClick={() => setWithdrawalEmployee(employees[0] ?? null)} disabled={employees.length === 0} icon={<Plus size={17} />}>إضافة مسحوب</Button>}>
            {withdrawals.length === 0 ? <EmptyState icon={<Banknote size={28} />} title="لا توجد مسحوبات في اليوم المحدد" description="سجّل أول مسحوب وسيُحتسب المتبقي تلقائيًا." /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>التاريخ</th><th>الموظف</th><th>المبلغ</th><th>ملاحظات</th><th>سجله</th><th>إجراءات</th></tr></thead><tbody>{withdrawals.map((withdrawal) => <tr key={withdrawal.id}><td>{dateFormat(withdrawal.withdrawn_at)}</td><td><strong>{withdrawal.employee_name}</strong></td><td className="money-cell">{money(withdrawal.amount)}</td><td>{withdrawal.notes || '—'}</td><td>{withdrawal.created_by_name || '—'}</td><td><div className="row-actions"><button onClick={() => setEditingWithdrawal(withdrawal)} aria-label="تعديل المسحوب"><Pencil size={15} /></button><button className="danger-action" onClick={() => void removeWithdrawal(withdrawal)} aria-label="حذف المسحوب"><Trash2 size={15} /></button></div></td></tr>)}</tbody></table></div>}
          </SectionCard>
          <SectionCard title="سجل الخصومات" subtitle={`سجل الخصومات ليوم ${workingDateFormat(selectedDate)}.`}>
            {deductions.length === 0 ? <EmptyState icon={<ReceiptText size={28} />} title="لا توجد خصومات في اليوم المحدد" description="استخدم زر خصم بجانب الموظف لتسجيل أول خصم." /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>التاريخ</th><th>الموظف</th><th>المبلغ</th><th>السبب / الملاحظات</th><th>سجله</th><th>إجراءات</th></tr></thead><tbody>{deductions.map((deduction) => <tr key={deduction.id}><td>{dateFormat(deduction.deducted_at)}</td><td><strong>{deduction.employee_name}</strong></td><td className="money-cell">{money(deduction.amount)}</td><td>{deduction.notes || '—'}</td><td>{deduction.created_by_name || '—'}</td><td><div className="row-actions"><button onClick={() => setEditingDeduction(deduction)} aria-label="تعديل الخصم"><Pencil size={15} /></button><button className="danger-action" onClick={() => void removeDeduction(deduction)} aria-label="حذف الخصم"><Trash2 size={15} /></button></div></td></tr>)}</tbody></table></div>}
          </SectionCard>
        </>
      )}
      {salaryEmployee && <SalaryModal employee={salaryEmployee} month={month} onClose={() => setSalaryEmployee(null)} onSaved={() => { setSalaryEmployee(null); void load(); onNotify({ tone: 'success', text: 'تم حفظ المرتب الشهري وتحديث الإجماليات.' }); }} onNotify={onNotify} />}
      {addEmployeeOpen && <AddSalaryEmployeeModal defaultMonth={month} onClose={() => setAddEmployeeOpen(false)} onSaved={(savedMonth) => { setAddEmployeeOpen(false); if (savedMonth === month) void load(); onNotify({ tone: 'success', text: 'تمت إضافة الموظف إلى المرتبات وحفظ راتبه الشهري.' }); }} onNotify={onNotify} />}
      {withdrawalEmployee && <SalaryWithdrawalModal employees={employees} month={month} selectedEmployee={withdrawalEmployee} onClose={() => setWithdrawalEmployee(null)} onSaved={() => { setWithdrawalEmployee(null); void load(); onNotify({ tone: 'success', text: 'تم تسجيل المسحوب وإعادة احتساب الراتب المتبقي.' }); }} onNotify={onNotify} />}
      {deductionEmployee && <SalaryDeductionModal employees={employees} selectedEmployee={deductionEmployee} month={month} onClose={() => setDeductionEmployee(null)} onSaved={() => { setDeductionEmployee(null); void load(); onNotify({ tone: 'success', text: 'تم تسجيل الخصم وإعادة احتساب الراتب المتبقي.' }); }} onNotify={onNotify} />}
      {editingWithdrawal && <SalaryWithdrawalModal employees={employees} month={month} withdrawal={editingWithdrawal} onClose={() => setEditingWithdrawal(null)} onSaved={() => { setEditingWithdrawal(null); void load(); onNotify({ tone: 'success', text: 'تم تعديل المسحوب وإعادة احتساب الإجماليات.' }); }} onNotify={onNotify} />}
      {editingDeduction && <SalaryDeductionModal employees={employees} deduction={editingDeduction} month={month} onClose={() => setEditingDeduction(null)} onSaved={() => { setEditingDeduction(null); void load(); onNotify({ tone: 'success', text: 'تم تعديل الخصم وإعادة احتساب الراتب المتبقي.' }); }} onNotify={onNotify} />}
    </>
  );
}

function AddSalaryEmployeeModal({ defaultMonth, onClose, onSaved, onNotify }: { defaultMonth: string; onClose: () => void; onSaved: (month: string) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ full_name: '', salary: '', month: defaultMonth });
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try {
      await api.createPayrollEmployee({ full_name: form.full_name.trim(), month: form.month, salary: form.salary });
      onSaved(form.month);
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    } finally { setSaving(false); }
  };
  return <Modal title="إضافة موظف إلى المرتبات" subtitle="أدخل بيانات الموظف يدويًا، دون إنشاء حساب مستخدم في النظام." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="اسم الموظف" required><input value={form.full_name} onChange={(event) => setForm((current) => ({ ...current, full_name: event.target.value }))} placeholder="اسم الموظف" required autoFocus /></FormField><div className="form-grid form-grid--two"><FormField label="المرتب الشهري (د.ل)" required><input value={form.salary} onChange={(event) => setForm((current) => ({ ...current, salary: event.target.value }))} type="number" min="0.001" step="0.001" required dir="ltr" /></FormField><FormField label="شهر المرتب" required><input value={form.month} onChange={(event) => setForm((current) => ({ ...current, month: event.target.value }))} type="month" required dir="ltr" /></FormField></div><div className="inline-alert inline-alert--info"><History size={17} /> سيُحفظ الموظف مباشرة في المرتبات، ويُحتسب المتبقي تلقائيًا بعد تسجيل المسحوبات.</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <UserPlus size={17} />}>{saving ? 'جارٍ الحفظ...' : 'إضافة الموظف'}</Button></div></form></Modal>;
}

function SalaryModal({ employee, month, onClose, onSaved, onNotify }: { employee: PayrollEmployee; month: string; onClose: () => void; onSaved: () => void; onNotify: (message: ToastMessage) => void }) {
  const [salary, setSalary] = useState(employee.salary_configured ? String(employee.salary) : '');
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try {
      await api.setEmployeeSalary(employee.employee_id, month, salary);
      onSaved();
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    } finally { setSaving(false); }
  };
  return <Modal title={`مرتب ${employee.employee_name}`} subtitle={`القيمة فعالة من شهر ${month} وتبقى للأشهر التالية حتى تغييرها.`} onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="المرتب الشهري (د.ل)" required><input value={salary} onChange={(event) => setSalary(event.target.value)} type="number" min="0.001" step="0.001" required dir="ltr" autoFocus /></FormField><div className="inline-alert inline-alert--info"><History size={17} /> لن تتغير قيم المرتبات المحفوظة للأشهر السابقة.</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ المرتب'}</Button></div></form></Modal>;
}

function SalaryWithdrawalModal({ employees, month, selectedEmployee, withdrawal, onClose, onSaved, onNotify }: { employees: PayrollEmployee[]; month: string; selectedEmployee?: PayrollEmployee; withdrawal?: SalaryWithdrawal; onClose: () => void; onSaved: () => void; onNotify: (message: ToastMessage) => void }) {
  const defaultDate = withdrawal?.withdrawn_at.slice(0, 10) ?? (month === monthInputValue() ? dateInputValue() : `${month}-01`);
  const [form, setForm] = useState({ employee_id: withdrawal?.employee_id ?? selectedEmployee?.employee_id ?? '', amount: withdrawal ? String(withdrawal.amount) : '', withdrawn_at: defaultDate, notes: withdrawal?.notes ?? '' });
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    const payload = { employee_id: form.employee_id, amount: form.amount, withdrawn_at: `${form.withdrawn_at}T12:00:00Z`, notes: form.notes.trim() || null };
    try {
      if (withdrawal) await api.updateSalaryWithdrawal(withdrawal.id, payload);
      else await api.createSalaryWithdrawal(payload);
      onSaved();
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    } finally { setSaving(false); }
  };
  return <Modal title={withdrawal ? 'تعديل مسحوب موظف' : 'تسجيل مسحوب موظف'} subtitle="يُخصم المبلغ تلقائيًا من مرتب شهر تاريخ المسحوب." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="الموظف" required><select value={form.employee_id} onChange={(event) => setForm((current) => ({ ...current, employee_id: event.target.value }))} required><option value="">اختر الموظف</option>{employees.map((employee) => <option key={employee.employee_id} value={employee.employee_id}>{employee.employee_name}</option>)}</select></FormField><div className="form-grid form-grid--two"><FormField label="المبلغ (د.ل)" required><input value={form.amount} onChange={(event) => setForm((current) => ({ ...current, amount: event.target.value }))} type="number" min="0.001" step="0.001" required dir="ltr" /></FormField><FormField label="التاريخ" required><input value={form.withdrawn_at} onChange={(event) => setForm((current) => ({ ...current, withdrawn_at: event.target.value }))} type="date" required dir="ltr" /></FormField></div><FormField label="ملاحظات"><textarea value={form.notes} onChange={(event) => setForm((current) => ({ ...current, notes: event.target.value }))} placeholder="اختياري" rows={3} /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : withdrawal ? 'حفظ التعديل' : 'تسجيل المسحوب'}</Button></div></form></Modal>;
}

function SalaryDeductionModal({ employees, month, selectedEmployee, deduction, onClose, onSaved, onNotify }: { employees: PayrollEmployee[]; month: string; selectedEmployee?: PayrollEmployee; deduction?: SalaryDeduction; onClose: () => void; onSaved: () => void; onNotify: (message: ToastMessage) => void }) {
  const defaultDate = deduction?.deducted_at.slice(0, 10) ?? (month === monthInputValue() ? dateInputValue() : `${month}-01`);
  const [form, setForm] = useState({ employee_id: deduction?.employee_id ?? selectedEmployee?.employee_id ?? '', amount: deduction ? String(deduction.amount) : '', deducted_at: defaultDate, notes: deduction?.notes ?? '' });
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    const payload = { employee_id: form.employee_id, amount: form.amount, deducted_at: `${form.deducted_at}T12:00:00Z`, notes: form.notes.trim() || null };
    try {
      if (deduction) await api.updateSalaryDeduction(deduction.id, payload);
      else await api.createSalaryDeduction(payload);
      onSaved();
    } catch (requestError) {
      onNotify({ tone: 'error', text: friendlyError(requestError) });
    } finally { setSaving(false); }
  };
  return <Modal title={deduction ? 'تعديل خصم موظف' : `خصم من مرتب ${selectedEmployee?.employee_name ?? ''}`} subtitle="يُخصم المبلغ من مرتب شهر تاريخ الخصم، ويبقى منفصلًا عن المسحوبات." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="الموظف" required><select value={form.employee_id} onChange={(event) => setForm((current) => ({ ...current, employee_id: event.target.value }))} required><option value="">اختر الموظف</option>{employees.map((employee) => <option key={employee.employee_id} value={employee.employee_id}>{employee.employee_name}</option>)}</select></FormField><div className="form-grid form-grid--two"><FormField label="مبلغ الخصم (د.ل)" required><input value={form.amount} onChange={(event) => setForm((current) => ({ ...current, amount: event.target.value }))} type="number" min="0.001" step="0.001" required dir="ltr" autoFocus /></FormField><FormField label="التاريخ" required><input value={form.deducted_at} onChange={(event) => setForm((current) => ({ ...current, deducted_at: event.target.value }))} type="date" required dir="ltr" /></FormField></div><FormField label="السبب أو الملاحظات"><textarea value={form.notes} onChange={(event) => setForm((current) => ({ ...current, notes: event.target.value }))} placeholder="اختياري" rows={3} /></FormField><div className="inline-alert inline-alert--info"><History size={17} /> سيُحتسب المتبقي تلقائيًا: المرتب ناقص المسحوبات والخصومات.</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : deduction ? 'حفظ التعديل' : 'حفظ الخصم'}</Button></div></form></Modal>;
}

function FinanceView({ selectedDate, onNotify }: { selectedDate: string; onNotify: (message: ToastMessage) => void }) {
  const [overview, setOverview] = useState<FinanceOverview>({});
  const [showrooms, setShowrooms] = useState<Showroom[]>([]);
  const [showroomPayments, setShowroomPayments] = useState<PaymentRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [tab, setTab] = useState<'overview' | 'showroomPayments'>('overview');
  const [modal, setModal] = useState<'showroomPayment' | null>(null);
  const [editingShowroomPayment, setEditingShowroomPayment] = useState<PaymentRecord | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      const [nextOverview, nextShowrooms, nextShowroomPayments] = await Promise.all([
        api.financeOverview({ date: selectedDate }),
        api.showrooms({ date: selectedDate, include_financials: true }),
        api.showroomPayments({ date: selectedDate, limit: 300 }),
      ]);
      setOverview(nextOverview);
      setShowrooms(nextShowrooms);
      setShowroomPayments(nextShowroomPayments);
    } catch (error) {
      onNotify({ tone: 'error', text: friendlyError(error) });
    } finally { setLoading(false); }
  };
  useEffect(() => {
    void load();
    const handleRefresh = () => { void load(); };
    window.addEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
    return () => window.removeEventListener(FINANCIAL_REFRESH_EVENT, handleRefresh);
  }, [selectedDate]);
  const deleteShowroomPayment = async (payment: PaymentRecord) => {
    if (!window.confirm('هل تريد حذف هذه الدفعة؟ سيعود المبلغ إلى رصيد المعرض.')) return;
    try {
      await api.deleteShowroomPayment(payment.id);
      refreshFinancialViews();
      onNotify({ tone: 'success', text: 'تم حذف الدفعة وإعادة احتساب رصيد المعرض.' });
      await load();
    } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
  };

  return (
    <>
      <PageHeader eyebrow="وصول الإدارة فقط" title="المركز المالي" description={`الإيرادات والحسابات المالية ليوم ${workingDateFormat(selectedDate)}، مع فصل واضح بين حصة الأعمال وحقوق العمال.`} actions={<StatusBadge tone="info"><LockKeyhole size={13} /> معلومات مالية محمية</StatusBadge>} />
      {loading ? <LoadingBlock label="جارٍ تحميل السجلات المالية المصرح بها..." /> : (
        <>
          <div className="finance-hero-grid">
            <MetricCard label="صافي ربح الأعمال" value={money(overview.net_business_profit)} note={overview.period_label || 'للفترة المحددة'} icon={<Sparkles size={22} />} tone="blue" />
            <MetricCard label="إجمالي إيراد الغسيل" value={money(overview.revenue)} note="نقدي وحسابات المعارض" icon={<ArrowUpLeft size={22} />} tone="teal" />
            <MetricCard label="مستحقات العمال" value={money(overview.worker_payables)} note="بعد الاستقطاعات المالية" icon={<UsersRound size={22} />} tone="violet" />
            <MetricCard label="ديون المعارض" value={money(overview.showroom_outstanding)} note="أرصدة مستحقة التحصيل" icon={<Building2 size={22} />} tone="amber" />
          </div>
          <section className="finance-breakdown glass-card">
            <div className="finance-breakdown__header"><div><p className="eyebrow">حساب الربحية</p><h2>صافي ربح الأعمال</h2></div><span className="calculation-note">يُستبعد نصيب العمال من المصروف المشترك من أرباح الأعمال.</span></div>
            <div className="calculation-row"><span>إجمالي إيراد الغسيل</span><strong>{money(overview.revenue)}</strong></div>
            <div className="calculation-row calculation-row--subtract"><span>عمولات العمال</span><strong>− {money(overview.worker_commissions)}</strong></div>
            <div className="calculation-row calculation-row--subtotal"><span>حصة الأعمال</span><strong>{money(overview.business_share)}</strong></div>
            <div className="calculation-row calculation-row--result"><span>صافي ربح الأعمال</span><strong>{money(overview.net_business_profit)}</strong></div>
          </section>
          <div className="finance-tabs" role="tablist">
            <button className={tab === 'overview' ? 'is-active' : ''} onClick={() => setTab('overview')}>الملخص</button>
            <button className={tab === 'showroomPayments' ? 'is-active' : ''} onClick={() => setTab('showroomPayments')}>دفعات المعارض</button>
          </div>
          {tab === 'overview' && <FinanceOverviewPanel overview={overview} />}
          {tab === 'showroomPayments' && <PaymentsPanel title="دفعات المعارض" records={showroomPayments} personLabel="المعرض" onAdd={() => setModal('showroomPayment')} onEdit={setEditingShowroomPayment} onDelete={(record) => { void deleteShowroomPayment(record); }} empty="لم تسجّل أي دفعة من المعارض بعد." />}
        </>
      )}
      {modal === 'showroomPayment' && <ShowroomPaymentModal showrooms={showrooms} onClose={() => setModal(null)} onNotify={onNotify} onSaved={(record) => { setShowroomPayments((current) => [record, ...current]); setModal(null); refreshFinancialViews(); onNotify({ tone: 'success', text: 'تم تسجيل دفعة المعرض وتحديث رصيده.' }); void load(); }} />}
      {editingShowroomPayment && <ShowroomPaymentModal payment={editingShowroomPayment} showrooms={showrooms} onClose={() => setEditingShowroomPayment(null)} onNotify={onNotify} onSaved={(record) => { setShowroomPayments((current) => current.map((item) => item.id === record.id ? record : item)); setEditingShowroomPayment(null); refreshFinancialViews(); onNotify({ tone: 'success', text: 'تم تعديل دفعة المعرض وتحديث رصيده.' }); void load(); }} />}
    </>
  );
}

function FinanceOverviewPanel({ overview }: { overview: FinanceOverview }) {
  return <SectionCard title="تركيبة الإيراد" className="finance-overview-panel"><div className="finance-overview-list"><div><span><Banknote size={17} /> إيراد نقدي</span><strong>{money(overview.cash_revenue)}</strong></div><div><span><Building2 size={17} /> إيراد على حساب المعارض</span><strong>{money(overview.showroom_revenue)}</strong></div><div><span><UsersRound size={17} /> عمولات العمال</span><strong>{money(overview.worker_commissions)}</strong></div></div></SectionCard>;
}

function PaymentsPanel({ title, records, personLabel, onAdd, onEdit, onDelete, empty }: { title: string; records: PaymentRecord[]; personLabel: string; onAdd: () => void; onEdit?: (record: PaymentRecord) => void; onDelete?: (record: PaymentRecord) => void; empty: string }) {
  return <SectionCard title={title} subtitle="كل دفعة تحفظ باسم المستخدم الذي سجلها." action={<Button onClick={onAdd} icon={<Plus size={17} />}>تسجيل دفعة</Button>}>
    {records.length === 0 ? <EmptyState icon={<Banknote size={28} />} title={empty} action={<Button onClick={onAdd} icon={<Plus size={17} />}>تسجيل دفعة</Button>} /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>{personLabel}</th><th>المبلغ</th><th>التاريخ</th><th>ملاحظات</th><th>سجله</th>{(onEdit || onDelete) && <th>إجراءات</th>}</tr></thead><tbody>{records.map((record) => <tr key={record.id}><td>{record.worker_name || record.showroom_name || '—'}</td><td className="money-cell">{money(record.amount)}</td><td>{dateFormat(record.paid_at || record.date, true)}</td><td>{record.notes || '—'}</td><td>{record.created_by_name || '—'}</td>{(onEdit || onDelete) && <td><div className="row-actions">{onEdit && <button type="button" onClick={() => onEdit(record)} aria-label="تعديل الدفعة" title="تعديل الدفعة"><Pencil size={15} /></button>}{onDelete && <button type="button" className="danger-action" onClick={() => onDelete(record)} aria-label="حذف الدفعة" title="حذف الدفعة"><Trash2 size={15} /></button>}</div></td>}</tr>)}</tbody></table></div>}
  </SectionCard>;
}

function ShowroomPaymentModal({ payment, showrooms, onClose, onSaved, onNotify }: { payment?: PaymentRecord; showrooms: Showroom[]; onClose: () => void; onSaved: (record: PaymentRecord) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ showroom_id: payment?.showroom_id ?? '', amount: payment ? String(payment.amount) : '', paid_at: (payment?.paid_at || dateInputValue()).slice(0, 10), notes: payment?.notes ?? '' });
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => { event.preventDefault(); setSaving(true); try { const payload = { ...form, paid_at: new Date(`${form.paid_at}T12:00:00`).toISOString(), notes: form.notes.trim() || null }; onSaved(payment ? await api.updateShowroomPayment(payment.id, payload) : await api.createShowroomPayment(payload)); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); } };
  return <Modal title={payment ? 'تعديل دفعة المعرض' : 'تسجيل دفعة من معرض'} subtitle="سيخصم المبلغ تلقائيًا من الرصيد المستحق للمعرض." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="المعرض" required><select value={form.showroom_id} onChange={(event) => setForm((current) => ({ ...current, showroom_id: event.target.value }))} required><option value="">اختر المعرض</option>{showrooms.map((showroom) => <option key={showroom.id} value={showroom.id}>{showroom.name}</option>)}</select></FormField><FormField label="المبلغ (د.ل)" required><input value={form.amount} onChange={(event) => setForm((current) => ({ ...current, amount: event.target.value }))} type="number" min="0.01" step="0.01" required dir="ltr" /></FormField><FormField label="التاريخ" required><input value={form.paid_at} onChange={(event) => setForm((current) => ({ ...current, paid_at: event.target.value }))} type="date" required dir="ltr" /></FormField><FormField label="ملاحظات"><textarea value={form.notes} onChange={(event) => setForm((current) => ({ ...current, notes: event.target.value }))} placeholder="اختياري" rows={3} /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : payment ? 'حفظ التعديل' : 'تسجيل الدفعة'}</Button></div></form></Modal>;
}

function ExpenseModal({ onClose, onSaved, onNotify }: { onClose: () => void; onSaved: (record: ExpenseRecord) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ description: '', amount: '', spent_at: dateInputValue(), notes: '', allocation: 'business_only' as ExpenseAllocation });
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try {
      onSaved(await api.createExpense({ description: form.description.trim(), amount: form.amount, spent_at: new Date(form.spent_at).toISOString(), notes: form.notes.trim() || null, allocation: form.allocation, business_percentage: form.allocation === 'shared' ? 50 : 100 }));
    } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); }
  };
  return <Modal title="تسجيل مصروف" subtitle="يُحتسب التوزيع ويحفظ كسجل قابل للتدقيق." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="وصف المصروف" required><input value={form.description} onChange={(event) => setForm((current) => ({ ...current, description: event.target.value }))} placeholder="مثال: شراء مواد تنظيف" required /></FormField><div className="form-grid form-grid--two"><FormField label="المبلغ (د.ل)" required><input value={form.amount} onChange={(event) => setForm((current) => ({ ...current, amount: event.target.value }))} type="number" min="0.01" step="0.01" required dir="ltr" /></FormField><FormField label="التاريخ" required><input value={form.spent_at} onChange={(event) => setForm((current) => ({ ...current, spent_at: event.target.value }))} type="date" required dir="ltr" /></FormField></div><FormField label="تخصيص المصروف" required><select value={form.allocation} onChange={(event) => setForm((current) => ({ ...current, allocation: event.target.value as ExpenseAllocation }))}><option value="business_only">المركز فقط</option><option value="shared">المركز والعمال (50/50)</option></select></FormField>{form.allocation === 'shared' && <div className="allocation-preview"><span>التوزيع التلقائي</span><strong>50% للمركز — 50% للعمال</strong><small>توزّع حصة العمال بالتساوي على العمال النشطين وتحفظ كلقطة تاريخية.</small></div>}<FormField label="ملاحظات"><textarea value={form.notes} onChange={(event) => setForm((current) => ({ ...current, notes: event.target.value }))} placeholder="اختياري" rows={3} /></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'تسجيل المصروف'}</Button></div></form></Modal>;
}

function ExpensesView({selectedDate,onNotify}:{selectedDate:string;onNotify:(message:ToastMessage)=>void}){
  const[items,setItems]=useState<ExpenseRecord[]>([]);const[loading,setLoading]=useState(true);const[adding,setAdding]=useState(false);const[selected,setSelected]=useState<ExpenseRecord|null>(null);const[editing,setEditing]=useState<ExpenseRecord|null>(null);
  const load=async()=>{setLoading(true);try{setItems(await api.expenses({date:selectedDate}));}catch(error){onNotify({tone:'error',text:friendlyError(error)});}finally{setLoading(false);}};useEffect(()=>{void load();},[selectedDate]);
  const totals=items.reduce((sum,item)=>({total:sum.total+safeNumber(item.amount),business:sum.business+safeNumber(item.business_amount),workers:sum.workers+safeNumber(item.workers_amount)}),{total:0,business:0,workers:0});
  const openDetail=async(item:ExpenseRecord)=>{try{setSelected(await api.expense(item.id));}catch(error){onNotify({tone:'error',text:friendlyError(error)});}};
  const remove=async(item:ExpenseRecord)=>{if(!window.confirm(`هل تريد حذف المصروف «${item.description}»؟ سيتم عكس جميع استقطاعات العمال المرتبطة به.`))return;try{await api.deleteExpense(item.id);setItems(current=>current.filter(record=>record.id!==item.id));setSelected(null);setEditing(null);onNotify({tone:'success',text:'تم حذف المصروف وعكس آثاره المالية.'});}catch(error){onNotify({tone:'error',text:friendlyError(error)});}};
  return <><PageHeader eyebrow="وصول الإدارة فقط" title="المصروفات" description={`مصروفات يوم ${workingDateFormat(selectedDate)} وحصة المركز واستقطاعات العمال.`} actions={<Button onClick={()=>setAdding(true)} icon={<Plus size={17}/>}>تسجيل مصروف</Button>}/><SectionCard className="filter-card"><div className="filter-bar"><div className="date-filter"><CalendarDays size={17}/><span>تاريخ العمل:</span><strong>{workingDateFormat(selectedDate)}</strong></div></div></SectionCard><div className="metric-grid"><MetricCard label="إجمالي المصروفات" value={money(totals.total)} note="خلال اليوم المحدد" icon={<ReceiptText size={21}/>} tone="blue"/><MetricCard label="حصة المركز" value={money(totals.business)} note="تُخصم من صافي ربح المركز" icon={<Building2 size={21}/>} tone="amber"/><MetricCard label="حصة العمال" value={money(totals.workers)} note="استقطاعات موزعة على العمال" icon={<UsersRound size={21}/>} tone="violet"/></div><SectionCard title="سجل المصروفات" subtitle="اضغط على أي سجل لعرض لقطة العمال والتفاصيل الكاملة.">{loading?<LoadingBlock/>:items.length===0?<EmptyState icon={<ReceiptText size={28}/>} title="لا توجد مصروفات في اليوم المحدد"/>:<div className="data-table-wrap"><table className="data-table"><thead><tr><th>التاريخ</th><th>الوصف</th><th>الإجمالي</th><th>التوزيع</th><th>حصة المركز</th><th>حصة العمال</th><th>سجله</th><th>إجراءات</th></tr></thead><tbody>{items.map(item=><tr key={item.id} className="clickable-row" onClick={()=>void openDetail(item)}><td>{dateFormat(item.spent_at)}</td><td><strong>{item.description}</strong></td><td className="money-cell">{money(item.amount)}</td><td>{item.allocation==='shared'?'المركز والعمال':'المركز فقط'}</td><td>{money(item.business_amount)}</td><td>{money(item.workers_amount)}</td><td>{item.created_by_name||'—'}</td><td><div className="row-actions"><button onClick={e=>{e.stopPropagation();setEditing(item)}}><Pencil size={15}/></button><button className="danger-action" onClick={e=>{e.stopPropagation();void remove(item)}}><Trash2 size={15}/></button></div></td></tr>)}</tbody></table></div>}</SectionCard>{adding&&<ExpenseModal onClose={()=>setAdding(false)} onSaved={()=>{setAdding(false);void load();onNotify({tone:'success',text:'تم تسجيل المصروف وتوزيعه.'});}} onNotify={onNotify}/>} {editing&&<EditExpenseModal expense={editing} onClose={()=>setEditing(null)} onSaved={()=>{setEditing(null);setSelected(null);void load();onNotify({tone:'success',text:'تم تحديث المصروف وإعادة توزيع آثاره.'});}} onNotify={onNotify}/>} {selected&&<ExpenseDetails expense={selected} onClose={()=>setSelected(null)} onEdit={()=>{setEditing(selected);setSelected(null)}} onDelete={()=>void remove(selected)}/>}</>;
}

function EditExpenseModal({expense,onClose,onSaved,onNotify}:{expense:ExpenseRecord;onClose:()=>void;onSaved:()=>void;onNotify:(message:ToastMessage)=>void}){const[form,setForm]=useState({description:expense.description,amount:String(expense.amount),spent_at:(expense.spent_at??'').slice(0,10),notes:expense.notes??'',allocation:expense.allocation==='shared'?'shared':'business_only'});const[saving,setSaving]=useState(false);const submit=async(e:FormEvent)=>{e.preventDefault();setSaving(true);try{await api.updateExpense(expense.id,{...form,spent_at:new Date(form.spent_at).toISOString(),notes:form.notes.trim()||null});onSaved();}catch(error){onNotify({tone:'error',text:friendlyError(error)});}finally{setSaving(false);}};return <Modal title="تعديل المصروف" subtitle="تظل لقطة العمال الأصلية ثابتة عند إعادة توزيع المصروف المشترك." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="الوصف" required><input value={form.description} onChange={e=>setForm(c=>({...c,description:e.target.value}))} required/></FormField><div className="form-grid form-grid--two"><FormField label="المبلغ" required><input value={form.amount} onChange={e=>setForm(c=>({...c,amount:e.target.value}))} type="number" min="0.01" step="0.01" required/></FormField><FormField label="التاريخ"><input value={form.spent_at} onChange={e=>setForm(c=>({...c,spent_at:e.target.value}))} type="date" required/></FormField></div><FormField label="التوزيع"><select value={form.allocation} onChange={e=>setForm(c=>({...c,allocation:e.target.value}))}><option value="business_only">المركز فقط</option><option value="shared">المركز والعمال (50/50)</option></select></FormField><FormField label="ملاحظات"><textarea value={form.notes} onChange={e=>setForm(c=>({...c,notes:e.target.value}))}/></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={<Save size={17}/>}>{saving?'جارٍ الحفظ...':'حفظ التعديل'}</Button></div></form></Modal>}

function ExpenseDetails({expense,onClose,onEdit,onDelete}:{expense:ExpenseRecord;onClose:()=>void;onEdit:()=>void;onDelete:()=>void}){return <SidePanel title="تفاصيل المصروف" onClose={onClose}><div className="profile-heading"><div className="profile-heading__avatar profile-heading__avatar--showroom"><ReceiptText size={25}/></div><div><h2>{expense.description}</h2><span>{dateFormat(expense.spent_at,true)}</span></div></div><div className="mini-metric-grid"><MiniMetric label="إجمالي المصروف" value={expense.amount}/><MiniMetric label="حصة المركز" value={expense.business_amount} tone="warning"/><MiniMetric label="حصة العمال" value={expense.workers_amount} tone="danger"/></div><div className="profile-actions"><Button variant="secondary" onClick={onEdit} icon={<Pencil size={16}/>}>تعديل</Button><Button variant="danger" onClick={onDelete} icon={<Trash2 size={16}/>}>حذف</Button></div><SectionCard title="العمال المشاركون" subtitle={`${expense.allocations?.length??0} عامل في اللقطة التاريخية`}>{expense.allocations?.length?<div className="finance-overview-list">{expense.allocations.map(item=><div key={item.worker_id}><span><UsersRound size={16}/>{item.worker_name}</span><strong>{money(item.amount)}</strong></div>)}</div>:<EmptyState title="لا توجد استقطاعات عمال لهذا المصروف"/>}</SectionCard>{expense.notes&&<SectionCard title="ملاحظات"><p>{expense.notes}</p></SectionCard>}</SidePanel>}

function ReportsView({ selectedDate, isManager }: { selectedDate: string; isManager: boolean }) {
  const [range, setRange] = useState<DateRange>(() => selectedDateRange(selectedDate));
  const [operational, setOperational] = useState<OperationalReport | null>(null);
  const [financial, setFinancial] = useState<FinancialReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = async (nextRange = range) => {
    setLoading(true); setError(null);
    try {
      const parameters = { from: nextRange.from, to: nextRange.to };
      const operationalReport = await api.operationalReport(parameters);
      setOperational(operationalReport);
      if (isManager) setFinancial(await api.financialReport(parameters));
      else setFinancial(null);
    } catch (requestError) { setError(friendlyError(requestError)); } finally { setLoading(false); }
  };
  useEffect(() => {
    const nextRange = selectedDateRange(selectedDate);
    setRange(nextRange);
    void load(nextRange);
  }, [isManager, selectedDate]);
  const workerRows = isManager ? financial?.worker_performance : operational?.workers;

  return (
    <>
      <PageHeader eyebrow={isManager ? 'تقارير الإدارة' : 'التقارير التشغيلية'} title="التقارير" description={isManager ? 'تقارير تشغيلية ومالية تفصيلية بحسب الفترة المختارة.' : 'تقارير تشغيلية للمركبات والعمال دون أي بيانات مالية.'} />
      <SectionCard className="report-filter-card"><div className="report-filter"><div><CalendarDays size={20} /><div><strong>الفترة الزمنية</strong><span>اختر التاريخين ثم حدّث التقرير</span></div></div><input type="date" value={range.from} onChange={(event) => setRange((current) => ({ ...current, from: event.target.value }))} dir="ltr" /><span>إلى</span><input type="date" value={range.to} onChange={(event) => setRange((current) => ({ ...current, to: event.target.value }))} dir="ltr" /><Button className="report-refresh-button" onClick={() => void load()} icon={<RefreshCw size={17} />}>تحديث التقرير</Button></div></SectionCard>
      {loading ? <LoadingBlock label="جارٍ إعداد التقرير..." /> : error ? <InlineRetry error={error} onRetry={() => void load()} /> : (
        <>
          <div className="report-headline glass-card"><div className="report-headline__icon"><FileText size={25} /></div><div><span>الفترة المعروضة</span><strong>{dateFormat(range.from)} — {dateFormat(range.to)}</strong></div><div><span>إجمالي السيارات المغسولة</span><strong>{operational?.cars_washed ?? operational?.washes?.length ?? 0} سيارة</strong></div>{isManager && <div><span>صافي ربح الأعمال</span><strong>{money(financial?.net_business_profit)}</strong></div>}</div>
          {isManager && financial && <div className="report-finance-grid"><MetricCard label="إجمالي الإيراد" value={money(financial.revenue)} icon={<ArrowUpLeft size={20} />} /><MetricCard label="حصة الأعمال" value={money(financial.business_share)} icon={<Banknote size={20} />} tone="teal" /><MetricCard label="عمولات العمال" value={money(financial.worker_commissions)} icon={<UsersRound size={20} />} tone="violet" /><MetricCard label="مصروفات الأعمال" value={money(financial.business_expenses)} icon={<ReceiptText size={20} />} tone="amber" /></div>}
          <div className="report-grid">
            <SectionCard title="أداء العمال" subtitle={isManager ? 'عدد السيارات والمؤشرات المالية لكل عامل.' : 'عدد السيارات المغسولة لكل عامل ضمن الفترة.'}>
              {!workerRows?.length ? <EmptyState icon={<UsersRound size={27} />} title="لا توجد بيانات أداء ضمن هذه الفترة" /> : <div className="data-table-wrap"><table className="data-table"><thead><tr><th>العامل</th><th>السيارات المغسولة</th>{isManager && <><th>العمولات</th><th>الاستقطاعات</th><th>المستحق</th></>}</tr></thead><tbody>{workerRows.map((row) => {
                const financialRow = row as { commissions?: number | string; deductions?: number | string; payable?: number | string };
                return <tr key={row.worker_id}><td>{row.worker_name}</td><td>{row.cars_washed}</td>{isManager && <><td className="money-cell">{money(financialRow.commissions)}</td><td className="money-cell">{money(financialRow.deductions)}</td><td className="money-cell">{money(financialRow.payable)}</td></>}</tr>;
              })}</tbody></table></div>}
            </SectionCard>
            <SectionCard title="عمليات الغسيل" subtitle="المركبات المسجلة خلال الفترة المحددة.">
              {operational?.washes?.length ? <WashList washes={operational.washes.slice(0, 8)} compact /> : <EmptyState icon={<Car size={27} />} title="لا توجد عمليات ضمن هذه الفترة" />}
            </SectionCard>
          </div>
          {isManager && financial && <ManagerReportDetails report={financial} />}
        </>
      )}
    </>
  );
}

function ManagerReportDetails({ report }: { report: FinancialReport }) {
  return <section className="report-manager-details"><SectionCard title="تفاصيل ديون المعارض"><div className="finance-overview-list"><div><span><Building2 size={17} /> إيراد حسابات المعارض</span><strong>{money(report.showroom_revenue)}</strong></div><div><span><Banknote size={17} /> مدفوعات المعارض</span><strong>{money(report.showroom_payments?.reduce((total, payment) => total + safeNumber(payment.amount), 0))}</strong></div><div><span><AlertTriangle size={17} /> الدين المستحق</span><strong>{money(report.showroom_outstanding)}</strong></div></div></SectionCard><SectionCard title="توزيع المصروفات"><div className="finance-overview-list"><div><span><ReceiptText size={17} /> إجمالي المصروفات</span><strong>{money(report.total_expenses)}</strong></div><div><span><Banknote size={17} /> حصة الأعمال</span><strong>{money(report.business_expenses)}</strong></div><div><span><UsersRound size={17} /> حصة العمال</span><strong>{money(report.workers_expenses)}</strong></div></div></SectionCard></section>;
}

function SettingsView({ user, canSettings, canUsers, onNotify }: { user: AuthUser; canSettings: boolean; canUsers: boolean; onNotify: (message: ToastMessage) => void }) {
  const [settings, setSettings] = useState<AppSettings>({ business_name: BUSINESS_NAME, currency: 'د.ل', default_worker_commission_percentage: 50 });
  const [users, setUsers] = useState<AuthUser[]>([]);
  const [roles, setRoles] = useState<Role[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [addUserOpen, setAddUserOpen] = useState(false);
  const [editingUser, setEditingUser] = useState<AuthUser | null>(null);
  const [permissionUser, setPermissionUser] = useState<AuthUser | null>(null);
  const [roleEditor, setRoleEditor] = useState<Role | null>(null);
  const deletingUserIds = useRef(new Set<string>());

  const load = async () => {
    setLoading(true);
    try {
      const [nextSettings, nextUsers, nextRoles] = await Promise.all([
        canSettings ? api.settings() : Promise.resolve(null),
        canUsers ? api.users() : Promise.resolve([]),
        canUsers ? api.roles() : Promise.resolve([]),
      ]);
      if (nextSettings) setSettings((current) => ({ ...current, ...nextSettings }));
      if (canUsers) { setUsers(nextUsers); setRoles(nextRoles); }
    } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, [canSettings, canUsers]);
  const saveSettings = async (event: FormEvent) => {
    event.preventDefault();
    const commission = safeNumber(settings.default_worker_commission_percentage);
    if (commission < 0 || commission > 100) { onNotify({ tone: 'error', text: 'يجب أن تكون نسبة العمولة بين 0 و100.' }); return; }
    setSaving(true);
    try { setSettings(await api.updateSettings({ business_name: settings.business_name, currency: settings.currency, default_worker_commission_percentage: commission })); onNotify({ tone: 'success', text: 'تم حفظ إعدادات المركز ونسبة العمولة الافتراضية.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); }
  };
  const updateUserStatus = async (target: AuthUser) => {
    const disabled = target.status === 'disabled';
    try {
      const updated = await api.updateUser(target.id, { status: disabled ? 'active' : 'disabled' });
      setUsers((current) => current.map((person) => person.id === updated.id ? updated : person));
      onNotify({ tone: 'success', text: disabled ? 'تم تفعيل حساب المستخدم.' : 'تم تعطيل حساب المستخدم.' });
    } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
  };
  const deleteUser = async (target: AuthUser) => {
    if (!managerOf(user) || target.role === 'manager') return;
    if (deletingUserIds.current.has(target.id)) return;
    if (!window.confirm(`هل تريد حذف حساب المستخدم ${target.full_name}؟ سيبقى سجله التاريخي محفوظًا.`)) return;
    deletingUserIds.current.add(target.id);
    try {
      await api.deleteUser(target.id);
      setUsers((current) => current.filter((person) => person.id !== target.id));
      await load();
      onNotify({ tone: 'success', text: 'تم حذف حساب المستخدم مع الاحتفاظ بسجلاته التاريخية.' });
    } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); }
    finally { deletingUserIds.current.delete(target.id); }
  };
  return (
    <>
      <PageHeader eyebrow="وصول الإدارة فقط" title="الإعدادات والإدارة" description="إدارة هوية المركز والعمولات والمستخدمين والأدوار والصلاحيات." actions={<StatusBadge tone="info"><ShieldCheck size={13} /> إدارة محمية</StatusBadge>} />
      {loading ? <LoadingBlock /> : <div className="settings-grid">
        {canSettings && <SectionCard title="إعدادات المركز" subtitle="هذه القيم تستخدم في التقارير والحسابات الجديدة." className="settings-grid__wide"><form className="entry-form" onSubmit={saveSettings}><div className="form-grid form-grid--three"><FormField label="اسم المركز" required><input value={settings.business_name ?? ''} onChange={(event) => setSettings((current) => ({ ...current, business_name: event.target.value }))} required /></FormField><FormField label="العملة" required><input value={settings.currency ?? 'د.ل'} onChange={(event) => setSettings((current) => ({ ...current, currency: event.target.value }))} required /></FormField><FormField label="نسبة عمولة العامل الافتراضية (%)" required hint="تطبّق تلقائيًا على العمليات الجديدة وفق إعدادات النظام."><input value={settings.default_worker_commission_percentage ?? ''} onChange={(event) => setSettings((current) => ({ ...current, default_worker_commission_percentage: event.target.value }))} type="number" min="0" max="100" step="0.01" required dir="ltr" /></FormField></div><div className="form-actions"><p><LockKeyhole size={16} /> لا تظهر نسب العمولات إلا للحسابات المخولة ماليًا.</p><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ الإعدادات'}</Button></div></form></SectionCard>}
        {canUsers && <SectionCard title="المستخدمون" subtitle="إنشاء الحسابات وتعطيلها وتعيين الأدوار والتحكم بصلاحيات كل موظف." className="settings-grid__wide" action={<Button onClick={() => setAddUserOpen(true)} icon={<UserPlus size={17} />}>إضافة مستخدم</Button>}><div className="data-table-wrap"><table className="data-table"><thead><tr><th>الاسم</th><th>اسم المستخدم</th><th>الدور</th><th>الحالة</th><th>إجراء</th></tr></thead><tbody>{users.map((person) => <tr key={person.id}><td>{person.full_name}</td><td dir="ltr">{person.username}</td><td>{person.role_name || (person.role === 'manager' ? 'مدير' : 'موظف')}</td><td><StatusBadge tone={person.status === 'disabled' ? 'danger' : 'success'}>{person.status === 'disabled' ? 'معطل' : 'نشط'}</StatusBadge></td><td><button className="table-action" onClick={() => setEditingUser(person)}><Pencil size={15} /> تعديل</button>{managerOf(user) && person.role !== 'manager' && <button className="table-action" onClick={() => setPermissionUser(person)}><ShieldCheck size={15} /> الصلاحيات</button>}{person.id !== user.id && <button className="table-action" onClick={() => void updateUserStatus(person)}>{person.status === 'disabled' ? 'تفعيل' : 'تعطيل'}</button>}{managerOf(user) && person.role !== 'manager' && <button className="table-action danger-action" onClick={() => void deleteUser(person)} title="حذف المستخدم" aria-label={`حذف المستخدم ${person.full_name}`}><Trash2 size={15} /> حذف</button>}</td></tr>)}</tbody></table></div></SectionCard>}
        {canUsers && <SectionCard title="الأدوار والصلاحيات" subtitle="تُفرض الصلاحيات من الخادم قبل عرض أي صفحة أو تنفيذ أي إجراء." className="settings-grid__wide"><div className="role-list">{roles.map((role) => <div className="role-row" key={role.id}><div className="role-row__icon"><ShieldCheck size={19} /></div><div><strong>{role.name}</strong><span>{role.key === 'manager' ? 'صلاحيات المدير كاملة وثابتة' : 'تحكم دقيق فيما يراه الموظفون وما يمكنهم تنفيذه'}</span><small>{role.key === 'manager' ? 'وصول كامل' : `${role.permissions?.length ?? 0} صلاحية مفعّلة`}</small></div>{managerOf(user) && role.key !== 'manager' && <button className="table-action" onClick={() => setRoleEditor(role)}><Pencil size={15} /> إدارة الصلاحيات</button>}</div>)}</div></SectionCard>}
      </div>}
      {addUserOpen && <CreateUserModal onClose={() => setAddUserOpen(false)} roles={roles} onSaved={(created) => { setUsers((current) => [created, ...current]); setAddUserOpen(false); onNotify({ tone: 'success', text: 'تم إنشاء المستخدم وتعيين دوره.' }); }} onNotify={onNotify} />}
      {editingUser && <EditUserModal user={editingUser} roles={roles} onClose={() => setEditingUser(null)} onSaved={(updated) => { setUsers((current) => current.map((person) => person.id === updated.id ? updated : person)); setEditingUser(null); onNotify({ tone: 'success', text: 'تم تحديث حساب المستخدم.' }); }} onNotify={onNotify} />}
      {permissionUser && <EmployeePermissionsModal user={permissionUser} onClose={() => setPermissionUser(null)} onSaved={(updated) => { setUsers((current) => current.map((person) => person.id === updated.id ? updated : person)); setPermissionUser(null); onNotify({ tone: 'success', text: 'تم حفظ صلاحيات الموظف.' }); }} onNotify={onNotify} />}
      {roleEditor && <RoleEditorModal role={roleEditor} onClose={() => setRoleEditor(null)} onSaved={(updated) => { setRoles((current) => current.map((role) => role.id === updated.id ? updated : role)); setRoleEditor(null); onNotify({ tone: 'success', text: 'تم حفظ صلاحيات الدور.' }); }} onNotify={onNotify} />}
    </>
  );
}

function CreateUserModal({ roles, onClose, onSaved, onNotify }: { roles: Role[]; onClose: () => void; onSaved: (user: AuthUser) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ full_name: '', username: '', password: '', role: roles.find((role) => role.key === 'employee')?.key || 'employee' });
  const [saving, setSaving] = useState(false);
  const update = (key: keyof typeof form, value: string) => setForm((current) => ({ ...current, [key]: value }));
  const submit = async (event: FormEvent) => { event.preventDefault(); setSaving(true); try { onSaved(await api.createUser(form)); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); } };
  return <Modal title="إضافة مستخدم" subtitle="لا يمكن للمستخدمين تغيير أدوارهم أو ترقيتها بأنفسهم." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="الاسم الكامل" required><input value={form.full_name} onChange={(event) => update('full_name', event.target.value)} required /></FormField><FormField label="اسم المستخدم" required><input value={form.username} onChange={(event) => update('username', event.target.value)} required dir="ltr" /></FormField><FormField label="كلمة مرور أولية" required><input value={form.password} onChange={(event) => update('password', event.target.value)} type="password" minLength={8} required dir="ltr" /></FormField><FormField label="الدور" required><select value={form.role} onChange={(event) => update('role', event.target.value)}>{roles.map((role) => <option key={role.id} value={role.key}>{role.name}</option>)}</select></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <UserPlus size={17} />}>{saving ? 'جارٍ الإنشاء...' : 'إنشاء المستخدم'}</Button></div></form></Modal>;
}

function EditUserModal({ user, roles, onClose, onSaved, onNotify }: { user: AuthUser; roles: Role[]; onClose: () => void; onSaved: (user: AuthUser) => void; onNotify: (message: ToastMessage) => void }) {
  const [form, setForm] = useState({ full_name: user.full_name, username: user.username, password: '', role: user.role, status: user.status || 'active' });
  const [saving, setSaving] = useState(false);
  const update = (key: keyof typeof form, value: string) => setForm((current) => ({ ...current, [key]: value }));
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (form.password && form.password.length < 10) { onNotify({ tone: 'error', text: 'يجب ألا تقل كلمة المرور الجديدة عن 10 أحرف.' }); return; }
    setSaving(true);
    try { onSaved(await api.updateUser(user.id, { full_name: form.full_name.trim(), username: form.username.trim(), password: form.password || undefined, role: form.role === user.role ? undefined : form.role, status: form.status === user.status ? undefined : form.status })); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); }
  };
  return <Modal title="تعديل حساب مستخدم" subtitle="اترك كلمة المرور فارغة إذا لم ترغب في تغييرها." onClose={onClose}><form className="entry-form" onSubmit={submit}><FormField label="الاسم الكامل" required><input value={form.full_name} onChange={(event) => update('full_name', event.target.value)} required /></FormField><FormField label="اسم المستخدم" required><input value={form.username} onChange={(event) => update('username', event.target.value)} required dir="ltr" /></FormField><FormField label="كلمة مرور جديدة"><input value={form.password} onChange={(event) => update('password', event.target.value)} type="password" minLength={10} placeholder="اتركها فارغة دون تغيير" dir="ltr" /></FormField><FormField label="الدور" required><select value={form.role} onChange={(event) => update('role', event.target.value)}>{roles.map((role) => <option key={role.id} value={role.key}>{role.name}</option>)}</select></FormField><FormField label="حالة الحساب" required><select value={form.status} onChange={(event) => update('status', event.target.value)}><option value="active">نشط</option><option value="disabled">معطل</option></select></FormField><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ التغييرات'}</Button></div></form></Modal>;
}

function updatedPermissions(current: string[], code: PermissionCode) {
  const enabled = current.includes(code);
  let next = enabled ? current.filter((value) => value !== code) : [...current, code];
  if (code === 'operational.read' && enabled) next = next.filter((value) => value !== 'operational.write');
  if (code === 'operational.write' && !enabled && !next.includes('operational.read')) next.push('operational.read');
  return next;
}

function PermissionToggleList({ permissions, onToggle }: { permissions: string[]; onToggle: (code: PermissionCode) => void }) {
  return <div className="permission-toggle-list">{PERMISSION_OPTIONS.map((permission) => { const enabled = permissions.includes(permission.code); return <div className="permission-toggle-row" key={permission.code}><div><strong>{permission.name}</strong><span>{permission.description}</span></div><button type="button" className={`permission-switch ${enabled ? 'is-on' : ''}`} role="switch" aria-checked={enabled} onClick={() => onToggle(permission.code)}><span>{enabled ? 'مفعّل' : 'متوقف'}</span><i aria-hidden="true" /></button></div>; })}</div>;
}

function EmployeePermissionsModal({ user, onClose, onSaved, onNotify }: { user: AuthUser; onClose: () => void; onSaved: (user: AuthUser) => void; onNotify: (message: ToastMessage) => void }) {
  const [permissions, setPermissions] = useState<string[]>(user.permissions ?? []);
  const [saving, setSaving] = useState(false);
  const submit = async (event: FormEvent) => { event.preventDefault(); setSaving(true); try { onSaved(await api.updateUserPermissions(user.id, permissions)); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); } };
  return <Modal title={`صلاحيات ${user.full_name}`} subtitle="هذه الإعدادات خاصة بهذا الموظف وتحدد ما يراه وما يمكنه تنفيذه." onClose={onClose} wide><form className="entry-form" onSubmit={submit}><PermissionToggleList permissions={permissions} onToggle={(code) => setPermissions((current) => updatedPermissions(current, code))} /><div className="inline-alert inline-alert--info"><ShieldCheck size={17} /> تطبق التغييرات فورًا على جلسة الموظف الحالية وعلى كل طلب إلى الخادم.</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ الصلاحيات'}</Button></div></form></Modal>;
}

function RoleEditorModal({ role, onClose, onSaved, onNotify }: { role: Role; onClose: () => void; onSaved: (role: Role) => void; onNotify: (message: ToastMessage) => void }) {
  const [permissions, setPermissions] = useState<string[]>(role.permissions);
  const [saving, setSaving] = useState(false);
  const toggle = (code: PermissionCode) => setPermissions((current) => updatedPermissions(current, code));
  const submit = async (event: FormEvent) => { event.preventDefault(); setSaving(true); try { onSaved(await api.updateRole(role.id, { permissions })); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setSaving(false); } };
  return <Modal title={`صلاحيات ${role.name}`} subtitle="فعّل الصلاحيات الافتراضية للحسابات التابعة لهذا الدور." onClose={onClose} wide><form className="entry-form" onSubmit={submit}><PermissionToggleList permissions={permissions} onToggle={toggle} /><div className="inline-alert inline-alert--info"><ShieldCheck size={17} /> يمكن تخصيص صلاحيات كل موظف على حدة من جدول المستخدمين.</div><div className="modal-actions"><Button variant="ghost" onClick={onClose}>إلغاء</Button><Button type="submit" disabled={saving} icon={saving ? <LoaderCircle size={17} className="spin" /> : <Save size={17} />}>{saving ? 'جارٍ الحفظ...' : 'حفظ الصلاحيات'}</Button></div></form></Modal>;
}

function AuditView({ selectedDate }: { selectedDate: string }) {
  const [records, setRecords] = useState<AuditLog[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const load = async () => { setLoading(true); setError(null); try { setRecords(await api.auditLogs({ date: selectedDate, limit: 500 })); } catch (requestError) { setError(friendlyError(requestError)); } finally { setLoading(false); } };
  useEffect(() => { void load(); }, [selectedDate]);
  return <><PageHeader eyebrow="وصول الإدارة فقط" title="سجل التدقيق" description={`الأفعال المسجلة ليوم ${workingDateFormat(selectedDate)}، مرتبطة بالمستخدم والسجل المتأثر.`} actions={<Button variant="secondary" onClick={() => void load()} icon={<RefreshCw size={17} />}>تحديث</Button>} />{loading ? <LoadingBlock /> : error ? <InlineRetry error={error} onRetry={() => void load()} /> : <SectionCard title="أحداث اليوم المحدد" subtitle="لا يمكن تعديل السجل من واجهة التشغيل.">{records.length === 0 ? <EmptyState icon={<History size={28} />} title="لا توجد أحداث في اليوم المحدد" /> : <div className="audit-list">{records.map((record) => <article className="audit-row" key={record.id}><div className="audit-row__dot"><History size={16} /></div><div><h3>{record.action}</h3><p>{record.description || record.affected_record || 'تم تنفيذ إجراء في النظام.'}</p><span>{record.user_name || 'مستخدم النظام'} <i>•</i> {dateFormat(record.created_at, true)}</span></div></article>)}</div>}</SectionCard>}</>;
}

function BackupView({ selectedDate, onNotify }: { selectedDate: string; onNotify: (message: ToastMessage) => void }) {
  const [records, setRecords] = useState<BackupRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const load = async () => { setLoading(true); try { setRecords(await api.backups({ date: selectedDate })); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setLoading(false); } };
  useEffect(() => { void load(); }, [selectedDate]);
  const create = async () => { setCreating(true); try { const created = await api.createBackup(); await api.downloadBackup(created); await load(); onNotify({ tone: 'success', text: 'تم إنشاء النسخة الاحتياطية وتنزيلها بنجاح.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setCreating(false); } };
  const download = async (record: BackupRecord) => { try { await api.downloadBackup(record); onNotify({ tone: 'success', text: 'تم تنزيل النسخة الاحتياطية.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } };
  const remove = async (record: BackupRecord) => { if (!window.confirm('هل أنت متأكد من حذف هذه النسخة الاحتياطية؟ لا يمكن التراجع عن هذا الإجراء.')) return; try { await api.deleteBackup(record.id); setRecords((current) => current.filter((item) => item.id !== record.id)); onNotify({ tone: 'success', text: 'تم حذف ملف النسخة الاحتياطية نهائيًا.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } };
  const restore = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    const accepted = window.confirm('تحذير: ستستبدل الاستعادة جميع البيانات الحالية ببيانات النسخة المحددة. هل تريد المتابعة؟');
    if (!accepted) return;
    setRestoring(true);
    try { await api.restoreBackup(file); onNotify({ tone: 'success', text: 'تمت استعادة النسخة الاحتياطية. سجّل الدخول مجددًا لمتابعة العمل.' }); } catch (error) { onNotify({ tone: 'error', text: friendlyError(error) }); } finally { setRestoring(false); }
  };
  return <><PageHeader eyebrow="وصول الإدارة فقط" title="النسخ الاحتياطي والاستعادة" description="حافظ على نسخة آمنة من قاعدة البيانات المحلية، واستعدها فقط بعد التأكد." /><div className="backup-hero glass-card"><div className="backup-hero__icon"><DatabaseBackup size={29} /></div><div><h2>حماية بيانات مركزك</h2><p>تتضمن النسخة الاحتياطية قاعدة البيانات المحلية كاملة: العمليات والدفعات والمصروفات والإعدادات وسجل التدقيق.</p></div><Button onClick={() => void create()} disabled={creating} icon={creating ? <LoaderCircle size={18} className="spin" /> : <Download size={18} />}>{creating ? 'جارٍ إنشاء النسخة...' : 'إنشاء نسخة احتياطية'}</Button></div><div className="backup-grid"><SectionCard title="استعادة نسخة" subtitle="استخدم ملف قاعدة بيانات موثوقًا تم إنشاؤه من هذا النظام."><div className="restore-warning"><AlertTriangle size={22} /><div><strong>تنبيه مهم</strong><p>الاستعادة تستبدل البيانات الحالية بالكامل ولا يمكن التراجع عنها من داخل النظام.</p></div></div><input ref={fileRef} type="file" accept=".db,application/octet-stream" className="visually-hidden" onChange={(event) => void restore(event)} /><Button variant="danger" onClick={() => fileRef.current?.click()} disabled={restoring} icon={restoring ? <LoaderCircle size={17} className="spin" /> : <Upload size={17} />}>{restoring ? 'جارٍ الاستعادة...' : 'اختيار ملف للاستعادة'}</Button></SectionCard><SectionCard title="أفضل الممارسات"><div className="backup-tips"><span><CheckCircle2 size={17} /> أنشئ نسخة قبل أي استعادة.</span><span><CheckCircle2 size={17} /> احتفظ بنسخ في موقع آمن منفصل.</span><span><CheckCircle2 size={17} /> تحقق من تاريخ النسخة قبل استخدامها.</span></div></SectionCard></div><SectionCard title="سجل النسخ الاحتياطية" subtitle="النسخ المنشأة من النظام الحالي.">{loading ? <LoadingBlock /> : records.length === 0 ? <EmptyState icon={<DatabaseBackup size={28} />} title="لا توجد نسخ محفوظة بعد" description="أنشئ أول نسخة احتياطية الآن لحماية بياناتك." /> : <div className="backup-list">{records.map((record) => <div className="backup-row" key={record.id}><div className="backup-row__icon"><DatabaseBackup size={19} /></div><div><strong>{record.file_name || 'نسخة قاعدة بيانات'}</strong><span>{dateFormat(record.created_at, true)} {record.size_bytes ? `• ${formatNumber(record.size_bytes / 1024)} كيلوبايت` : ''}</span></div><div className="backup-row__actions"><button className="table-action" onClick={() => void download(record)}><Download size={15} /> تنزيل</button><button className="table-action danger-action" onClick={() => void remove(record)}><Trash2 size={15} /> حذف</button></div></div>)}</div>}</SectionCard></>;
}
