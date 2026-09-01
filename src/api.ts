import type {
  AppSettings,
  AuditLog,
  AuthUser,
  BackupRecord,
  DashboardData,
  DateRange,
  ExpenseRecord,
  FinanceOverview,
  FinancialReport,
  LoginResult,
  Money,
  OperationalReport,
  OvernightCar,
  PaidCarsData,
  PaymentRecord,
  PayrollSummary,
  Role,
  SalaryDeduction,
  SalaryWithdrawal,
  SetupStatus,
  Showroom,
  ShowroomDebtProfile,
  ShowroomDebtSummary,
  Wash,
  Worker,
  WorkerWithdrawalReturnLedger,
} from './types';

const TOKEN_KEY = 'alkaheli.session-token';
const isTauri = typeof window !== 'undefined' && ('__TAURI_INTERNALS__' in window || '__TAURI__' in window);
const API_ROOT = import.meta.env.VITE_API_URL ?? (isTauri ? 'http://127.0.0.1:8787/api' : '/api');

type RecordValue = Record<string, unknown>;

export class ApiError extends Error {
  readonly status: number;

  constructor(message: string, status = 0) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

function record(value: unknown): RecordValue {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as RecordValue : {};
}

function records(value: unknown): RecordValue[] {
  if (Array.isArray(value)) return value.map(record);
  const payload = record(value);
  return Array.isArray(payload.items) ? payload.items.map(record) : [];
}

function text(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : value === null || value === undefined ? fallback : String(value);
}

function number(value: unknown): number {
  const parsed = typeof value === 'number' ? value : Number(value ?? 0);
  return Number.isFinite(parsed) ? parsed : 0;
}

function money(value: unknown): number {
  return number(value) / 1000;
}

function toIsoEndOfDay(value: unknown, end = false): string | undefined {
  const source = text(value).trim();
  if (!source) return undefined;
  if (source.includes('T')) return source;
  const start = new Date(`${source}T00:00:00+02:00`);
  if (Number.isNaN(start.getTime())) return undefined;
  if (!end) return start.toISOString();
  return new Date(start.getTime() + 86_400_000 - 1).toISOString();
}

function query(params?: Record<string, string | number | boolean | undefined | null>) {
  if (!params) return '';
  const search = new URLSearchParams();
  Object.entries(params).forEach(([key, value]) => {
    if (value === undefined || value === null || value === '') return;
    if (key === 'from') search.set(key, toIsoEndOfDay(value) ?? '');
    else if (key === 'to') search.set(key, toIsoEndOfDay(value, true) ?? '');
    else search.set(key, String(value));
  });
  const output = search.toString();
  return output ? `?${output}` : '';
}

function mapUser(raw: unknown): AuthUser {
  const value = record(raw);
  return {
    id: text(value.id),
    full_name: text(value.fullName),
    username: text(value.username),
    role: text(value.roleCode, 'employee'),
    role_name: text(value.roleName),
    status: value.isActive === false ? 'disabled' : 'active',
    permissions: Array.isArray(value.permissions) ? value.permissions.map((permission) => text(permission)).filter(Boolean) : [],
    theme: text(value.theme, 'light'),
  };
}

function mapWash(raw: unknown): Wash {
  const value = record(raw);
  const worker = record(value.worker);
  const showroom = record(value.showroom);
  const creator = record(value.createdBy);
  return {
    id: text(value.id),
    vehicle_make: text(value.vehicleMake),
    vehicle_model: text(value.vehicleModel),
    car_color: value.carColor === null || value.carColor === undefined ? null : text(value.carColor),
    wash_type: value.washType === null || value.washType === undefined ? null : text(value.washType),
    manufacturing_year: value.manufactureYear === null || value.manufactureYear === undefined ? null : number(value.manufactureYear),
    license_plate: value.licensePlate === null || value.licensePlate === undefined ? null : text(value.licensePlate),
    price: value.priceMilli === undefined || value.priceMilli === null ? undefined : money(value.priceMilli),
    worker_due: value.commissionMilli === undefined || value.commissionMilli === null ? undefined : money(value.commissionMilli),
    worker_id: text(worker.id),
    worker_name: text(worker.fullName) || text(value.workerName),
    showroom_id: text(showroom.id) || null,
    showroom_name: text(showroom.name) || null,
    payment_type: value.paymentType === 'showroom' ? 'showroom_account' : 'cash',
    showroom_payment_method: value.showroomPaymentMethod === 'bank' ? 'bank' : value.showroomPaymentMethod === 'cash' ? 'cash' : null,
    performed_at: text(value.occurredAt),
    created_at: text(value.createdAt) || undefined,
    status: text(value.status, 'posted'),
    is_overnight: value.isOvernight === true,
    is_paid: value.isPaid === true,
    paid_at: value.paidAt === null || value.paidAt === undefined ? null : text(value.paidAt),
    created_by_id: text(creator.id) || undefined,
    created_by_name: text(creator.fullName) || undefined,
  };
}

function mapWorker(raw: unknown): Worker {
  const value = record(raw);
  const finance = record(value.financial);
  const hasFinance = Object.keys(finance).length > 0;
  return {
    id: text(value.id),
    name: text(value.fullName),
    phone: value.phone === null || value.phone === undefined ? null : text(value.phone),
    notes: value.notes === null || value.notes === undefined ? null : text(value.notes),
    status: value.isActive === false ? 'inactive' : 'active',
    washes_count: number(value.washCount),
    cars_washed: number(value.washCount),
    total_wash_value: money(value.totalWashValueMilli),
    financials: hasFinance ? {
      commission_percentage: value.commissionBpsOverride === null || value.commissionBpsOverride === undefined ? undefined : number(value.commissionBpsOverride) / 100,
      gross_commission: money(finance.grossCommissionMilli),
      deductions: money(finance.deductionsMilli),
      net_earnings: money(finance.grossCommissionMilli) - money(finance.deductionsMilli),
      amount_paid: money(finance.paidMilli),
      payable_balance: money(finance.remainingMilli),
    } : undefined,
    daily_value: value.dailyValue === null || value.dailyValue === undefined ? undefined : money(record(value.dailyValue).amountMilli),
    daily_value_date: value.dailyValue === null || value.dailyValue === undefined ? undefined : text(record(value.dailyValue).date) || undefined,
  };
}

function mapShowroom(raw: unknown): Showroom {
  const value = record(raw);
  const finance = record(value.financial);
  const hasFinance = Object.keys(finance).length > 0;
  return {
    id: text(value.id),
    name: text(value.name),
    contact_name: value.contactName === null || value.contactName === undefined ? null : text(value.contactName),
    phone: value.phone === null || value.phone === undefined ? null : text(value.phone),
    address: value.address === null || value.address === undefined ? (value.notes === null || value.notes === undefined ? null : text(value.notes)) : text(value.address),
    created_at: value.createdAt === null || value.createdAt === undefined ? null : text(value.createdAt),
    washes_count: number(value.washCount),
    financials: hasFinance ? {
      total_charges: money(finance.chargesMilli),
      total_payments: money(finance.paymentsMilli),
      outstanding_balance: money(finance.outstandingMilli),
    } : undefined,
  };
}

function mapPayment(raw: unknown): PaymentRecord {
  const value = record(raw);
  const worker = record(value.worker);
  const showroom = record(value.showroom);
  return {
    id: text(value.id),
    worker_id: text(worker.id) || undefined,
    worker_name: text(worker.fullName) || undefined,
    showroom_id: text(showroom.id) || undefined,
    showroom_name: text(showroom.name) || undefined,
    amount: money(value.amountMilli),
    paid_at: text(value.paidAt) || undefined,
    notes: value.notes === null || value.notes === undefined ? null : text(value.notes),
    created_by_name: text(value.recordedBy) || undefined,
  };
}

function mapSalaryWithdrawal(raw: unknown): SalaryWithdrawal {
  const value = record(raw);
  const employee = record(value.employee);
  return {
    id: text(value.id),
    employee_id: text(employee.id),
    employee_name: text(employee.fullName),
    amount: money(value.amountMilli),
    withdrawn_at: text(value.withdrawnAt),
    notes: value.notes === null || value.notes === undefined ? null : text(value.notes),
    created_by_name: text(value.recordedBy) || undefined,
    created_at: text(value.createdAt) || undefined,
    updated_at: text(value.updatedAt) || undefined,
  };
}

function mapSalaryDeduction(raw: unknown): SalaryDeduction {
  const value = record(raw);
  const employee = record(value.employee);
  return {
    id: text(value.id),
    employee_id: text(employee.id),
    employee_name: text(employee.fullName),
    amount: money(value.amountMilli),
    deducted_at: text(value.deductedAt),
    notes: value.notes === null || value.notes === undefined ? null : text(value.notes),
    created_by_name: text(value.recordedBy) || undefined,
    created_at: text(value.createdAt) || undefined,
    updated_at: text(value.updatedAt) || undefined,
  };
}

function mapExpense(raw: unknown): ExpenseRecord {
  const value = record(raw);
  const allocation = text(value.allocationType);
  return {
    id: text(value.id),
    description: text(value.description),
    amount: money(value.amountMilli),
    spent_at: text(value.occurredAt) || undefined,
    notes: value.notes === null || value.notes === undefined ? null : text(value.notes),
    allocation: allocation === 'business' ? 'business_only' : allocation === 'workers' ? 'workers_only' : 'shared',
    business_percentage: number(value.businessBps) / 100,
    workers_percentage: number(value.workersBps) / 100,
    business_amount: money(value.businessAmountMilli),
    workers_amount: money(value.workersAmountMilli),
    created_by_name: text(value.recordedBy) || undefined,
  };
}

function mapFinance(raw: unknown): FinanceOverview {
  const value = record(raw);
  return {
    revenue: money(value.totalWashRevenueMilli),
    cash_revenue: money(value.cashRevenueMilli),
    showroom_revenue: money(value.showroomRevenueMilli),
    worker_commissions: money(value.workerCommissionsMilli),
    worker_deductions: money(value.workerDeductionsMilli),
    worker_payables: money(value.outstandingWorkerBalancesMilli),
    business_share: money(value.businessShareMilli),
    total_expenses: money(value.expensesMilli),
    business_expenses: money(value.businessExpensesMilli),
    workers_expenses: money(value.workerExpensesMilli),
    net_business_profit: money(value.netBusinessProfitMilli),
    showroom_outstanding: money(value.outstandingShowroomDebtMilli),
  };
}

class ApiClient {
  private token: string | null = sessionStorage.getItem(TOKEN_KEY);

  getToken() { return this.token; }

  setToken(token: string | null) {
    this.token = token;
    if (token) sessionStorage.setItem(TOKEN_KEY, token);
    else sessionStorage.removeItem(TOKEN_KEY);
  }

  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const headers = new Headers(init.headers);
    if (this.token) headers.set('Authorization', `Bearer ${this.token}`);
    if (init.body && !(init.body instanceof FormData) && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, { ...init, headers });
    } catch {
      throw new ApiError('تعذّر الاتصال بخدمة النظام المحلية. تأكد من تشغيل التطبيق.', 0);
    }
    const payload: unknown = response.headers.get('content-type')?.includes('application/json') ? await response.json().catch(() => undefined) : undefined;
    if (!response.ok) {
      const source = record(payload);
      throw new ApiError(text(source.error || source.message, 'تعذر إتمام الطلب.'), response.status);
    }
    return record(payload).data as T;
  }

  private get<T>(path: string) { return this.request<T>(path); }
  private post<T>(path: string, data?: unknown) { return this.request<T>(path, { method: 'POST', body: data instanceof FormData ? data : data === undefined ? undefined : JSON.stringify(data) }); }
  private put<T>(path: string, data: unknown) { return this.request<T>(path, { method: 'PUT', body: JSON.stringify(data) }); }
  private patch<T>(path: string, data: unknown) { return this.request<T>(path, { method: 'PATCH', body: JSON.stringify(data) }); }
  private delete<T>(path: string) { return this.request<T>(path, { method: 'DELETE' }); }

  async setupStatus(): Promise<SetupStatus> {
    const value = record(await this.get('/setup/status'));
    return { setup_required: value.needsSetup === true };
  }

  async initialManager(data: { full_name: string; username: string; password: string; confirm_password?: string }): Promise<LoginResult> {
    const value = record(await this.post('/setup/initial-manager', { fullName: data.full_name, username: data.username, password: data.password }));
    return { token: text(value.token), user: mapUser(value.user) };
  }

  async login(data: { username: string; password: string }): Promise<LoginResult> {
    const value = record(await this.post('/auth/login', data));
    return { token: text(value.token), user: mapUser(value.user) };
  }

  async me(): Promise<AuthUser> { return mapUser(await this.get('/auth/me')); }
  async logout() { await this.post('/auth/logout'); }
  async updateTheme(theme: 'light' | 'dark') { await this.put('/preferences/theme', { theme }); }

  async dashboard(selectedDate?: string): Promise<DashboardData> {
    const value = record(await this.get(`/dashboard${query({ date: selectedDate })}`));
    const finance = record(value.financial);
    return {
      operational: { cars_today: number(value.todayWashes), cars_this_month: number(value.monthWashes), recent_washes: records(value.recentWashes).map(mapWash) },
      financial: Object.keys(finance).length ? {
        ...(Object.prototype.hasOwnProperty.call(finance, 'todayRevenue') ? { revenue_today: money(finance.todayRevenue) } : {}),
        ...(Object.prototype.hasOwnProperty.call(finance, 'todayCustomerRevenue') ? { customer_revenue_today: money(finance.todayCustomerRevenue) } : {}),
        ...(Object.prototype.hasOwnProperty.call(finance, 'todayNetProfit') ? { net_profit_today: money(finance.todayNetProfit) } : {}),
        ...(Object.prototype.hasOwnProperty.call(finance, 'todayShowroomRevenue') ? { showroom_revenue_today: money(finance.todayShowroomRevenue) } : {}),
        ...(Object.prototype.hasOwnProperty.call(finance, 'todayShowroomNetProfit') ? { showroom_net_profit_today: money(finance.todayShowroomNetProfit) } : {}),
        revenue_month: money(finance.monthRevenue), worker_payables: money(finance.workerPayable),
        expenses_month: money(finance.expenses), showroom_outstanding: money(finance.showroomOutstanding), business_share_month: money(finance.businessShare), net_profit_month: money(finance.netProfit),
      } : undefined,
    };
  }

  async washes(params?: Record<string, string | number | boolean | undefined | null>): Promise<Wash[]> { return records(await this.get(`/washes${query(params)}`)).map(mapWash); }

  async paidCars(params?: Record<string, string | number | boolean | undefined | null>): Promise<PaidCarsData> {
    const value = record(await this.get(`/paid-cars${query(params)}`));
    return { items: records(value.items).map(mapWash), settlement: money(value.settlementMilli) };
  }

  async setWashPaid(id: string, isPaid: boolean, selectedDate?: string): Promise<{ wash: Wash; settlement: number }> {
    const value = record(await this.patch(`/washes/${encodeURIComponent(id)}/paid${query({ date: selectedDate })}`, { isPaid }));
    return { wash: mapWash(value.wash), settlement: money(value.settlementMilli) };
  }

  async createWash(data: Record<string, unknown>): Promise<Wash> {
    const input = {
      vehicleMake: text(data.vehicle_make), vehicleModel: text(data.vehicle_model), manufactureYear: data.manufacturing_year ?? null,
      carColor: data.car_color ?? null,
      washType: data.wash_type ?? null,
      licensePlate: data.license_plate ?? null, price: text(data.price), workerId: text(data.worker_id),
      paymentType: data.payment_type === 'showroom_account' ? 'showroom' : 'cash', showroomId: data.showroom_id ?? null,
      showroomPaymentMethod: data.payment_type === 'showroom_account' ? data.showroom_payment_method ?? null : null,
      occurredAt: data.performed_at ?? undefined, clientRequestId: crypto.randomUUID(),
    };
    const saved = record(await this.post('/washes', input));
    const wash = record(saved.wash);
    if (Object.keys(wash).length > 0) return mapWash(wash);
    return {
      id: text(saved.id), vehicle_make: input.vehicleMake, vehicle_model: input.vehicleModel,
      car_color: input.carColor === null ? null : text(input.carColor),
      wash_type: input.washType === null ? null : text(input.washType),
      manufacturing_year: input.manufactureYear === null ? null : number(input.manufactureYear), license_plate: input.licensePlate === null ? null : text(input.licensePlate),
      price: number(input.price), worker_id: input.workerId, payment_type: input.paymentType === 'showroom' ? 'showroom_account' : 'cash', showroom_id: input.showroomId === null ? null : text(input.showroomId), showroom_payment_method: input.showroomPaymentMethod === 'bank' ? 'bank' : input.showroomPaymentMethod === 'cash' ? 'cash' : null, performed_at: text(input.occurredAt),
    };
  }

  /**
   * Expected manager-only contract: PATCH /api/washes/:id with the same
   * camelCase fields as POST /api/washes (without clientRequestId).
   * The service should recalculate the related financial entries atomically.
   */
  async updateWash(id: string, data: Record<string, unknown>): Promise<Wash> {
    const input = {
      vehicleMake: text(data.vehicle_make), vehicleModel: text(data.vehicle_model), manufactureYear: data.manufacturing_year ?? null,
      carColor: data.car_color ?? null,
      washType: data.wash_type ?? null,
      licensePlate: data.license_plate ?? null, price: text(data.price), workerId: text(data.worker_id),
      paymentType: data.payment_type === 'showroom_account' ? 'showroom' : 'cash', showroomId: data.showroom_id ?? null,
      showroomPaymentMethod: data.payment_type === 'showroom_account' ? data.showroom_payment_method ?? null : null,
      occurredAt: data.performed_at ?? undefined,
      markAsOvernight: data.mark_as_overnight === true,
    };
    const saved = record(await this.patch(`/washes/${encodeURIComponent(id)}`, input));
    const wash = record(saved.wash);
    if (Object.keys(wash).length > 0) return mapWash(wash);
    return {
      id,
      vehicle_make: input.vehicleMake,
      vehicle_model: input.vehicleModel,
      manufacturing_year: input.manufactureYear === null ? null : number(input.manufactureYear),
      license_plate: input.licensePlate === null ? null : text(input.licensePlate),
      car_color: input.carColor === null ? null : text(input.carColor),
      wash_type: input.washType === null ? null : text(input.washType),
      price: number(input.price),
      worker_id: input.workerId,
      showroom_id: input.showroomId === null ? null : text(input.showroomId),
      payment_type: input.paymentType === 'showroom' ? 'showroom_account' : 'cash',
      showroom_payment_method: input.showroomPaymentMethod === 'bank' ? 'bank' : input.showroomPaymentMethod === 'cash' ? 'cash' : null,
      performed_at: text(input.occurredAt),
      status: 'posted',
      is_overnight: input.markAsOvernight,
    };
  }

  async overnightCars(params?: Record<string, string | number | boolean | undefined | null>): Promise<OvernightCar[]> {
    return records(await this.get(`/overnight-cars${query(params)}`)).map((item) => ({
      id: text(item.id),
      wash: mapWash(item.wash),
      marked_at: text(item.markedAt),
      marked_by_name: text(item.markedBy) || undefined,
    }));
  }

  async deleteOvernightCar(id: string): Promise<void> { await this.delete(`/overnight-cars/${encodeURIComponent(id)}`); }

  /** Uses the existing audited reversal endpoint instead of physically deleting a financial record. */
  async voidWash(id: string, reason: string): Promise<void> {
    await this.post(`/washes/${encodeURIComponent(id)}/void`, { reason });
  }

  async workers(params?: Record<string, string | number | boolean | undefined | null>): Promise<Worker[]> { return records(await this.get(`/workers${query(params)}`)).map(mapWorker); }
  async createWorker(data: Record<string, unknown>): Promise<Worker> {
    const payload = { fullName: text(data.name || data.full_name), phone: data.phone ?? null };
    const saved = record(await this.post('/workers', payload));
    return { id: text(saved.id), name: payload.fullName, phone: payload.phone === null ? null : text(payload.phone), status: 'active', washes_count: 0 };
  }
  async updateWorker(id: string, data: Record<string, unknown>): Promise<Worker> {
    const commissionPercentage = data.commission_percentage;
    const payload = {
      fullName: text(data.name || data.full_name),
      phone: data.phone ?? null,
      notes: data.notes ?? null,
      isActive: data.status !== 'inactive',
      commissionBpsOverride: commissionPercentage === null || commissionPercentage === undefined || commissionPercentage === ''
        ? null
        : Math.round(number(commissionPercentage) * 100),
    };
    await this.patch(`/workers/${encodeURIComponent(id)}`, payload);
    return {
      id,
      name: payload.fullName,
      phone: payload.phone === null ? null : text(payload.phone),
      notes: payload.notes === null ? null : text(payload.notes),
      status: payload.isActive ? 'active' : 'inactive',
      financials: payload.commissionBpsOverride === null ? undefined : { commission_percentage: payload.commissionBpsOverride / 100 },
    };
  }
  async deleteWorker(id: string): Promise<void> { await this.delete(`/workers/${encodeURIComponent(id)}`); }
  async worker(id: string, includeFinancials: boolean, selectedDate?: string): Promise<Worker & { washes?: Wash[] }> {
    const detail = record(await this.get(`/workers/${encodeURIComponent(id)}${query({ date: selectedDate })}`));
    const worker = mapWorker(detail.worker);
    const dailyValue = record(detail.dailyValue);
    if (Object.keys(dailyValue).length > 0) {
      worker.daily_value = dailyValue.amountMilli === null || dailyValue.amountMilli === undefined ? undefined : money(dailyValue.amountMilli);
      worker.daily_value_date = text(dailyValue.date) || selectedDate;
    }
    const result: Worker & { washes?: Wash[] } = { ...worker, washes: records(detail.history).map(mapWash) };
    if (includeFinancials) {
      const financial = record(await this.get(`/workers/${encodeURIComponent(id)}/financial${query({ date: selectedDate })}`));
      result.financials = {
        commission_percentage: financial.commissionBpsOverride === null || financial.commissionBpsOverride === undefined ? undefined : number(financial.commissionBpsOverride) / 100,
        gross_commission: money(financial.grossCommissionMilli),
        deductions: money(financial.deductionsMilli),
        net_earnings: money(financial.netEarningsMilli),
        amount_paid: money(financial.paidMilli),
        payable_balance: money(financial.remainingMilli),
      };
    }
    return result;
  }
  async updateWorkerDailyValue(id: string, data: { value_date: string; amount: string }): Promise<{ amount: Money; date: string }> {
    const value = record(await this.put(`/workers/${encodeURIComponent(id)}/daily-value`, { valueDate: data.value_date, amount: data.amount }));
    return { amount: money(value.amountMilli), date: text(value.valueDate) };
  }
  async workerWithdrawalReturns(id: string, params?: Record<string, string | number | boolean | undefined | null>): Promise<WorkerWithdrawalReturnLedger> {
    const value = record(await this.get(`/workers/${encodeURIComponent(id)}/withdrawals-returns${query(params)}`));
    const worker = record(value.worker);
    return {
      worker_id: text(worker.id),
      worker_name: text(worker.fullName),
      total_withdrawals: money(value.totalWithdrawalsMilli),
      total_deductions: money(value.totalDeductionsMilli),
      total_returns: money(value.totalReturnsMilli),
      total_deduction_payments: money(value.totalDeductionPaymentsMilli),
      total_settlements: money(value.totalSettlementsMilli),
      outstanding_balance: money(value.outstandingBalanceMilli),
      transactions: records(value.transactions).map((transaction) => ({
        id: text(transaction.id),
        type: transaction.type === 'deduction' ? 'deduction' : transaction.type === 'deduction_payment' ? 'deduction_payment' : transaction.type === 'settlement' ? 'settlement' : transaction.type === 'return' ? 'return' : 'withdrawal',
        amount: money(transaction.amountMilli),
        occurred_at: text(transaction.occurredAt),
        notes: transaction.notes === null || transaction.notes === undefined ? null : text(transaction.notes),
        created_by_name: text(transaction.createdByName) || undefined,
        editable: transaction.editable === true,
        deletable: transaction.deletable !== false,
      })),
    };
  }
  async createWorkerWithdrawalReturn(id: string, data: { type: 'withdrawal' | 'return' | 'deduction_payment'; amount: string; occurred_at: string; notes?: string | null }): Promise<void> {
    await this.post(`/workers/${encodeURIComponent(id)}/withdrawals-returns`, {
      transactionType: data.type,
      amount: data.amount,
      occurredAt: data.occurred_at,
      notes: data.notes ?? null,
    });
  }
  async updateWorkerDeductionPayment(workerId: string, movementId: string, data: { amount: string; occurred_at: string; notes?: string | null }): Promise<void> {
    await this.patch(`/workers/${encodeURIComponent(workerId)}/withdrawals-returns/${encodeURIComponent(movementId)}`, {
      transactionType: 'deduction_payment',
      amount: data.amount,
      occurredAt: data.occurred_at,
      notes: data.notes ?? null,
    });
  }
  async settleWorkerWithdrawalReturns(id: string, occurredAt: string): Promise<void> {
    await this.post(`/workers/${encodeURIComponent(id)}/withdrawals-returns/settle`, { occurredAt });
  }
  async deleteWorkerWithdrawalReturn(workerId: string, movementId: string): Promise<void> {
    await this.delete(`/workers/${encodeURIComponent(workerId)}/withdrawals-returns/${encodeURIComponent(movementId)}`);
  }

  async showrooms(params?: Record<string, string | number | boolean | undefined | null>): Promise<Showroom[]> { return records(await this.get(`/showrooms${query(params)}`)).map(mapShowroom); }
  async showroomDebts(params?: Record<string, string | number | boolean | undefined | null>): Promise<ShowroomDebtSummary[]> {
    return records(await this.get(`/showroom-debts${query(params)}`)).map((item) => ({
      showroom: mapShowroom(item.showroom),
      outstanding_wash_count: number(item.outstandingWashCount),
      total_outstanding: money(item.totalOutstandingMilli),
      latest_wash_at: text(item.latestWashAt) || null,
    }));
  }
  async showroomDebt(id: string, range: DateRange): Promise<ShowroomDebtProfile> {
    const value = record(await this.get(`/showroom-debts/${encodeURIComponent(id)}${query({ from: range.from, to: range.to })}`));
    return {
      showroom: mapShowroom(value.showroom),
      from: text(value.from),
      to: text(value.to),
      outstanding_wash_count: number(value.outstandingWashCount),
      total_charges: money(value.totalChargesMilli),
      total_payments: money(value.totalPaymentsMilli),
      total_outstanding: money(value.totalOutstandingMilli),
      operations: records(value.operations).map(mapWash),
      payments: records(value.payments).map(mapPayment),
    };
  }
  async showroomStatistics(id: string, params: { from: string; to: string; paymentType: 'all' | 'cash' | 'debt' }): Promise<number> {
    const value = record(await this.get(`/showrooms/${encodeURIComponent(id)}/statistics${query(params)}`));
    return number(value.carCount);
  }
  async createShowroom(data: Record<string, unknown>): Promise<Showroom> {
    const payload = { name: text(data.name), contactName: data.contact_name ?? null, phone: data.phone ?? null, notes: data.address ?? null };
    const saved = record(await this.post('/showrooms', payload));
    return { id: text(saved.id), name: payload.name, contact_name: payload.contactName === null ? null : text(payload.contactName), phone: payload.phone === null ? null : text(payload.phone), washes_count: 0 };
  }
  async updateShowroom(id: string, data: Record<string, unknown>): Promise<Showroom> {
    const payload = { name: text(data.name), contactName: data.contact_name ?? null, phone: data.phone ?? null, notes: data.address ?? null };
    await this.patch(`/showrooms/${encodeURIComponent(id)}`, payload);
    return { id, name: payload.name, contact_name: payload.contactName === null ? null : text(payload.contactName), phone: payload.phone === null ? null : text(payload.phone), address: payload.notes === null ? null : text(payload.notes) };
  }
  async deleteShowroom(id: string): Promise<void> { await this.delete(`/showrooms/${encodeURIComponent(id)}`); }
  async showroom(id: string, includeFinancials: boolean, selectedDate?: string): Promise<Showroom & { washes?: Wash[] }> {
    const detail = record(await this.get(`/showrooms/${encodeURIComponent(id)}${query({ date: selectedDate })}`));
    const showroom = mapShowroom(detail.showroom);
    const result: Showroom & { washes?: Wash[] } = { ...showroom, washes: records(detail.history).map(mapWash) };
    if (includeFinancials) {
      const financial = record(await this.get(`/showrooms/${encodeURIComponent(id)}/financial${query({ date: selectedDate })}`));
      result.financials = { total_charges: money(financial.chargesMilli), total_payments: money(financial.paymentsMilli), outstanding_balance: money(financial.outstandingMilli) };
      result.payments = records(financial.payments).map((payment) => ({ ...mapPayment(payment), showroom_id: showroom.id, showroom_name: showroom.name }));
    }
    return result;
  }

  async financeOverview(params?: Record<string, string | number | boolean | undefined | null>): Promise<FinanceOverview> { return mapFinance(await this.get(`/finance/overview${query(params)}`)); }
  async payroll(month: string, selectedDate?: string): Promise<PayrollSummary> {
    const value = record(await this.get(`/payroll${query({ month, date: selectedDate })}`));
    return {
      month: text(value.month),
      employees: records(value.employees).map((item) => {
        const employee = record(item.employee);
        return {
          employee_id: text(employee.id),
          employee_name: text(employee.fullName),
          is_active: employee.isActive !== false,
          salary: money(item.salaryMilli),
          total_withdrawals: money(item.totalWithdrawalsMilli),
          total_deductions: money(item.totalDeductionsMilli),
          remaining_salary: money(item.remainingSalaryMilli),
          salary_configured: item.salaryConfigured === true,
        };
      }),
      total_salary: money(value.totalSalaryMilli),
      total_withdrawals: money(value.totalWithdrawalsMilli),
      total_deductions: money(value.totalDeductionsMilli),
      total_remaining: money(value.totalRemainingMilli),
    };
  }
  async setEmployeeSalary(employeeId: string, month: string, salary: string): Promise<void> {
    await this.put(`/payroll/employees/${encodeURIComponent(employeeId)}/salary`, { month, salary });
  }
  async createPayrollEmployee(data: { full_name: string; month: string; salary: string }): Promise<void> {
    await this.post('/payroll/employees', { fullName: data.full_name, month: data.month, salary: data.salary });
  }
  async deletePayrollEmployee(employeeId: string): Promise<void> {
    await this.delete(`/payroll/employees/${encodeURIComponent(employeeId)}`);
  }
  async salaryDeductions(month: string, selectedDate?: string): Promise<SalaryDeduction[]> {
    return records(await this.get(`/payroll/deductions${query({ month, date: selectedDate })}`)).map(mapSalaryDeduction);
  }
  async createSalaryDeduction(data: { employee_id: string; amount: string; deducted_at: string; notes?: string | null }): Promise<SalaryDeduction> {
    return mapSalaryDeduction(await this.post('/payroll/deductions', { employeeId: data.employee_id, amount: data.amount, deductedAt: data.deducted_at, notes: data.notes ?? null }));
  }
  async updateSalaryDeduction(id: string, data: { employee_id: string; amount: string; deducted_at: string; notes?: string | null }): Promise<SalaryDeduction> {
    return mapSalaryDeduction(await this.patch(`/payroll/deductions/${encodeURIComponent(id)}`, { employeeId: data.employee_id, amount: data.amount, deductedAt: data.deducted_at, notes: data.notes ?? null }));
  }
  async deleteSalaryDeduction(id: string): Promise<void> {
    await this.delete(`/payroll/deductions/${encodeURIComponent(id)}`);
  }
  async salaryWithdrawals(month: string, selectedDate?: string): Promise<SalaryWithdrawal[]> {
    return records(await this.get(`/payroll/withdrawals${query({ month, date: selectedDate })}`)).map(mapSalaryWithdrawal);
  }
  async createSalaryWithdrawal(data: { employee_id: string; amount: string; withdrawn_at: string; notes?: string | null }): Promise<SalaryWithdrawal> {
    return mapSalaryWithdrawal(await this.post('/payroll/withdrawals', {
      employeeId: data.employee_id,
      amount: data.amount,
      withdrawnAt: data.withdrawn_at,
      notes: data.notes ?? null,
    }));
  }
  async updateSalaryWithdrawal(id: string, data: { employee_id: string; amount: string; withdrawn_at: string; notes?: string | null }): Promise<SalaryWithdrawal> {
    return mapSalaryWithdrawal(await this.patch(`/payroll/withdrawals/${encodeURIComponent(id)}`, {
      employeeId: data.employee_id,
      amount: data.amount,
      withdrawnAt: data.withdrawn_at,
      notes: data.notes ?? null,
    }));
  }
  async deleteSalaryWithdrawal(id: string): Promise<void> {
    await this.delete(`/payroll/withdrawals/${encodeURIComponent(id)}`);
  }
  async showroomPayments(params?: Record<string, string | number | boolean | undefined | null>): Promise<PaymentRecord[]> { return records(await this.get(`/showroom-payments${query(params)}`)).map(mapPayment); }
  async createWorkerPayment(data: Record<string, unknown>): Promise<PaymentRecord> {
    const payload = { workerId: text(data.worker_id), amount: text(data.amount), paidAt: data.paid_at ?? undefined, notes: data.notes ?? null };
    const saved = record(await this.post('/worker-payments', payload));
    return { id: text(saved.id), worker_id: payload.workerId, amount: number(payload.amount), paid_at: text(payload.paidAt), notes: payload.notes === null ? null : text(payload.notes) };
  }
  async createShowroomPayment(data: Record<string, unknown>): Promise<PaymentRecord> {
    const payload = { showroomId: text(data.showroom_id), amount: text(data.amount), paidAt: data.paid_at ?? undefined, notes: data.notes ?? null };
    const saved = record(await this.post('/showroom-payments', payload));
    return mapPayment(saved);
  }
  async updateShowroomPayment(id: string, data: Record<string, unknown>): Promise<PaymentRecord> {
    return mapPayment(await this.patch(`/showroom-payments/${encodeURIComponent(id)}`, {
      showroomId: data.showroom_id,
      amount: text(data.amount),
      paidAt: data.paid_at ?? undefined,
      notes: data.notes ?? null,
    }));
  }
  async deleteShowroomPayment(id: string): Promise<void> {
    await this.delete(`/showroom-payments/${encodeURIComponent(id)}`);
  }
  async expenses(params?: Record<string, string | number | boolean | undefined | null>): Promise<ExpenseRecord[]> { return records(await this.get(`/expenses${query(params)}`)).map(mapExpense); }
  async createExpense(data: Record<string, unknown>): Promise<ExpenseRecord> {
    const allocation = text(data.allocation);
    const payload = { description: text(data.description), category: 'أخرى', amount: text(data.amount), occurredAt: data.spent_at ?? undefined, notes: data.notes ?? null, allocationType: allocation === 'business_only' ? 'business' : allocation === 'workers_only' ? 'workers' : 'shared', businessBps: Math.round(number(data.business_percentage) * 100) };
    const saved = record(await this.post('/expenses', payload));
    return { id: text(saved.id), description: payload.description, amount: number(payload.amount), spent_at: text(payload.occurredAt), notes: payload.notes === null ? null : text(payload.notes), allocation: allocation as ExpenseRecord['allocation'], business_percentage: payload.businessBps / 100, workers_percentage: 100 - payload.businessBps / 100, business_amount: money(saved.businessAmountMilli), workers_amount: money(saved.workersAmountMilli) };
  }
  async expense(id: string): Promise<ExpenseRecord> {
    const value = record(await this.get(`/expenses/${encodeURIComponent(id)}`));
    const expense = mapExpense(value.expense);
    expense.created_at = text(record(value.expense).createdAt) || undefined;
    expense.allocations = records(value.allocations).map((item) => ({ worker_id: text(item.workerId), worker_name: text(item.workerName), amount: money(item.amountMilli), created_at: text(item.createdAt) || undefined }));
    return expense;
  }
  async updateExpense(id: string, data: Record<string, unknown>): Promise<void> {
    const allocation = data.allocation === 'shared' ? 'shared' : 'business';
    await this.patch(`/expenses/${encodeURIComponent(id)}`, { description: data.description, category: 'أخرى', amount: text(data.amount), occurredAt: data.spent_at, notes: data.notes ?? null, allocationType: allocation, businessBps: allocation === 'shared' ? 5000 : 10000 });
  }
  async deleteExpense(id: string): Promise<void> { await this.delete(`/expenses/${encodeURIComponent(id)}`); }

  async operationalReport(params?: Record<string, string | number | boolean | undefined | null>): Promise<OperationalReport> {
    const value = record(await this.get(`/reports/operational${query(params)}`));
    return { cars_washed: number(value.carsWashed), washes: records(value.washes).map(mapWash), workers: records(value.workerPerformance).map((worker) => ({ worker_id: text(worker.workerId), worker_name: text(worker.workerName), cars_washed: number(worker.carsWashed) })) };
  }
  async financialReport(params?: Record<string, string | number | boolean | undefined | null>): Promise<FinancialReport> {
    const value = record(await this.get(`/reports/financial${query(params)}`));
    const summary = mapFinance(value.summary);
    const rawSummary = record(value.summary);
    return { ...summary, worker_performance: records(value.workerPerformance).map((worker) => ({ worker_id: text(worker.workerId), worker_name: text(worker.workerName), cars_washed: number(worker.carsWashed), commissions: money(worker.commissionMilli), deductions: money(worker.deductionsMilli), payable: money(worker.remainingMilli) })), showroom_payments: money(rawSummary.showroomPaymentsMilli) > 0 ? [{ id: 'total', amount: money(rawSummary.showroomPaymentsMilli) }] : [] };
  }

  async settings(): Promise<AppSettings> {
    const value = record(await this.get('/settings'));
    return { business_name: text(record(value.business_name).value), currency: text(record(value.currency).value, 'د.ل'), default_worker_commission_percentage: number(record(value.default_worker_commission_bps).value) / 100 };
  }
  async updateSettings(data: Record<string, unknown>): Promise<AppSettings> {
    await this.put('/settings', { businessName: data.business_name, currency: data.currency, defaultWorkerCommissionBps: Math.round(number(data.default_worker_commission_percentage) * 100) });
    return this.settings();
  }

  async users(): Promise<AuthUser[]> { return records(await this.get('/users')).map(mapUser); }
  async createUser(data: Record<string, unknown>): Promise<AuthUser> {
    const saved = record(await this.post('/users', { fullName: data.full_name, username: data.username, password: data.password, roleCode: data.role, isActive: true }));
    return (await this.users()).find((user) => user.id === text(saved.id)) ?? { id: text(saved.id), full_name: text(data.full_name), username: text(data.username), role: text(data.role, 'employee'), status: 'active' };
  }
  async updateUser(id: string, data: Record<string, unknown>): Promise<AuthUser> {
    const payload: RecordValue = {};
    if (data.status) payload.isActive = data.status === 'active';
    if (data.full_name) payload.fullName = data.full_name;
    if (data.username) payload.username = data.username;
    if (data.password) payload.password = data.password;
    if (data.role) payload.roleCode = data.role;
    await this.patch(`/users/${encodeURIComponent(id)}`, payload);
    return (await this.users()).find((user) => user.id === id) ?? { id, full_name: '', username: '', role: 'employee' };
  }
  async deleteUser(id: string): Promise<void> { await this.delete(`/users/${encodeURIComponent(id)}`); }
  async profilePicture(id: string): Promise<Blob | null> {
    const response = await fetch(`${API_ROOT}/users/${encodeURIComponent(id)}/profile-picture`, { headers: this.token ? { Authorization: `Bearer ${this.token}` } : undefined });
    if (response.status === 404) return null;
    if (!response.ok) throw new ApiError('تعذر تحميل صورة الحساب.', response.status);
    return response.blob();
  }
  async uploadProfilePicture(id: string, file: File): Promise<void> {
    const form = new FormData();
    form.append('file', file, file.name);
    const response = await fetch(`${API_ROOT}/users/${encodeURIComponent(id)}/profile-picture`, { method: 'PUT', headers: this.token ? { Authorization: `Bearer ${this.token}` } : undefined, body: form });
    const payload = response.headers.get('content-type')?.includes('application/json') ? await response.json().catch(() => undefined) : undefined;
    if (!response.ok) { const source = record(payload); throw new ApiError(text(source.error || source.message, 'تعذر تحديث صورة الحساب.'), response.status); }
  }
  async deleteProfilePicture(id: string): Promise<void> { await this.delete(`/users/${encodeURIComponent(id)}/profile-picture`); }
  async updateUserPermissions(id: string, permissions: string[]): Promise<AuthUser> {
    await this.put(`/users/${encodeURIComponent(id)}/permissions`, { permissionCodes: permissions });
    return (await this.users()).find((user) => user.id === id) ?? { id, full_name: '', username: '', role: 'employee', permissions };
  }
  async roles(): Promise<Role[]> { return records(await this.get('/roles')).map((role) => ({ id: text(role.id), key: text(role.code), name: text(role.name), permissions: records(role.permissions).map((permission) => text(permission.code)) })); }
  async updateRole(id: string, data: Record<string, unknown>): Promise<Role> {
    const permissions = Array.isArray(data.permissions) ? data.permissions.map((value) => text(value)) : [];
    await this.put(`/roles/${encodeURIComponent(id)}/permissions`, { permissionCodes: permissions });
    return (await this.roles()).find((role) => role.id === id) ?? { id, key: '', name: '', permissions };
  }

  async auditLogs(params?: Record<string, string | number | boolean | undefined | null>): Promise<AuditLog[]> { return records(await this.get(`/audit-logs${query(params)}`)).map((entry) => ({ id: text(entry.id), action: text(entry.action), description: text(entry.description) || undefined, affected_record: text(entry.entityId) || undefined, created_at: text(entry.createdAt), user_name: text(entry.userName) || undefined })); }
  async backups(params?: Record<string, string | number | boolean | undefined | null>): Promise<BackupRecord[]> { return records(await this.get(`/backups${query(params)}`)).map((entry) => ({ id: text(entry.id), file_name: text(entry.path).split(/[\\/]/).pop(), created_at: text(entry.createdAt), path: text(entry.path) || undefined, size_bytes: number(entry.sizeBytes), download_url: text(entry.downloadUrl) || undefined })); }
  async createBackup(path?: string): Promise<BackupRecord> {
    const saved = record(await this.post('/backups', path ? { path } : {}));
    return { id: text(saved.id), file_name: text(saved.path).split(/[\\/]/).pop(), path: text(saved.path), created_at: text(saved.createdAt, new Date().toISOString()), size_bytes: number(saved.sizeBytes), download_url: text(saved.downloadUrl) || undefined };
  }
  async chooseBackupPath(): Promise<string | undefined | null> {
    if (!isTauri) return undefined;
    const { save } = await import('@tauri-apps/plugin-dialog');
    return save({ title: 'حفظ نسخة احتياطية', defaultPath: `alkaheli-backup-${new Date().toISOString().slice(0, 10)}.db`, filters: [{ name: 'قاعدة بيانات SQLite', extensions: ['db'] }] });
  }
  async restoreBackup(file: File | string): Promise<void> {
    if (typeof file === 'string') {
      await this.post('/backups/restore', { path: file, confirmation: 'RESTORE' });
      return;
    }
    const form = new FormData(); form.append('backup', file); form.append('confirmation', 'RESTORE');
    await this.post('/backups/restore-upload', form);
  }
  async downloadBackup(backup: BackupRecord): Promise<void> {
    const filename = backup.file_name?.endsWith('.db') ? backup.file_name : `alkaheli-backup-${backup.id}.db`;
    if (isTauri) {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({ title: 'تنزيل النسخة الاحتياطية', defaultPath: filename, filters: [{ name: 'قاعدة بيانات SQLite', extensions: ['db'] }] });
      if (!path) return;
      await this.put(`/backups/${encodeURIComponent(backup.id)}/export`, { path });
      return;
    }
    const headers = new Headers();
    if (this.token) headers.set('Authorization', `Bearer ${this.token}`);
    const fallbackPath = `/backups/${encodeURIComponent(backup.id)}/download`;
    const suppliedPath = backup.download_url || fallbackPath;
    const downloadUrl = /^https?:\/\//.test(suppliedPath)
      ? suppliedPath
      : suppliedPath.startsWith('/api/')
        ? `${API_ROOT.replace(/\/api\/?$/, '')}${suppliedPath}`
        : `${API_ROOT}${suppliedPath.startsWith('/') ? suppliedPath : `/${suppliedPath}`}`;
    const response = await fetch(downloadUrl, { headers });
    if (!response.ok) {
      const payload = record(await response.json().catch(() => undefined));
      throw new ApiError(text(payload.error, 'تعذر تنزيل النسخة الاحتياطية.'), response.status);
    }
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement('a');
    link.href = url; link.download = filename; document.body.appendChild(link); link.click(); link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async deleteBackup(id: string): Promise<void> { await this.delete(`/backups/${encodeURIComponent(id)}`); }
}

export const api = new ApiClient();
