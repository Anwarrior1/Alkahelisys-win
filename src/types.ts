export type RoleKey = 'manager' | 'employee' | string;

export interface AuthUser {
  id: string;
  full_name: string;
  username: string;
  email?: string | null;
  role: RoleKey;
  role_name?: string;
  status?: 'active' | 'disabled' | string;
  permissions?: string[];
  theme?: 'light' | 'dark' | 'system' | string;
}

export interface SetupStatus {
  configured?: boolean;
  initialized?: boolean;
  has_users?: boolean;
  setup_required?: boolean;
}

export interface LoginResult {
  token: string;
  user: AuthUser;
}

export interface WorkerFinancials {
  commission_percentage?: number | string;
  gross_commission?: Money;
  deductions?: Money;
  net_earnings?: Money;
  amount_paid?: Money;
  payable_balance?: Money;
}

export interface Worker {
  id: string;
  name: string;
  phone?: string | null;
  notes?: string | null;
  status?: 'active' | 'inactive' | string;
  washes_count?: number;
  cars_washed?: number;
  total_wash_value?: Money;
  latest_wash_at?: string | null;
  financials?: WorkerFinancials;
  daily_value?: Money;
  daily_value_date?: string;
}

export interface ShowroomFinancials {
  total_charges?: Money;
  total_payments?: Money;
  outstanding_balance?: Money;
}

export interface Showroom {
  id: string;
  name: string;
  contact_name?: string | null;
  phone?: string | null;
  address?: string | null;
  created_at?: string | null;
  washes_count?: number;
  latest_wash_at?: string | null;
  financials?: ShowroomFinancials;
  payments?: PaymentRecord[];
}

export interface ShowroomDebtSummary {
  showroom: Showroom;
  outstanding_wash_count: number;
  total_outstanding: Money;
  latest_wash_at?: string | null;
}

export interface ShowroomDebtProfile {
  showroom: Showroom;
  from: string;
  to: string;
  outstanding_wash_count: number;
  total_charges: Money;
  total_payments: Money;
  total_outstanding: Money;
  operations: Wash[];
  payments: PaymentRecord[];
}

export type Money = number | string;

export type PaymentType = 'cash' | 'showroom_account';

export interface Wash {
  id: string;
  vehicle_make: string;
  vehicle_model: string;
  car_color?: string | null;
  wash_type?: string | null;
  manufacturing_year?: number | null;
  license_plate?: string | null;
  price?: Money;
  worker_due?: Money;
  worker_id?: string;
  worker_name?: string;
  showroom_id?: string | null;
  showroom_name?: string | null;
  payment_type: PaymentType;
  showroom_payment_method?: 'cash' | 'bank' | null;
  performed_at: string;
  created_at?: string;
  status?: 'posted' | 'voided' | string;
  is_overnight?: boolean;
  is_paid?: boolean;
  paid_at?: string | null;
  created_by_id?: string;
  created_by_name?: string;
}

export interface PaidCarsData {
  items: Wash[];
  settlement: Money;
}

export interface OvernightCar {
  id: string;
  wash: Wash;
  marked_at: string;
  marked_by_name?: string;
}

export interface DashboardData {
  operational: {
    cars_today?: number;
    cars_this_month?: number;
    recent_washes?: Wash[];
    active_workers?: number;
  };
  financial?: {
    revenue_today?: Money;
    customer_revenue_today?: Money;
    net_profit_today?: Money;
    showroom_revenue_today?: Money;
    showroom_net_profit_today?: Money;
    revenue_month?: Money;
    worker_payables?: Money;
    expenses_month?: Money;
    showroom_outstanding?: Money;
    business_share_month?: Money;
    net_profit_month?: Money;
  };
}

export interface FinanceOverview {
  revenue?: Money;
  cash_revenue?: Money;
  showroom_revenue?: Money;
  worker_commissions?: Money;
  worker_deductions?: Money;
  worker_payables?: Money;
  business_share?: Money;
  total_expenses?: Money;
  business_expenses?: Money;
  workers_expenses?: Money;
  net_business_profit?: Money;
  showroom_outstanding?: Money;
  period_label?: string;
}

export interface PaymentRecord {
  id: string;
  worker_id?: string;
  worker_name?: string;
  showroom_id?: string;
  showroom_name?: string;
  amount: Money;
  paid_at?: string;
  date?: string;
  notes?: string | null;
  created_by_name?: string;
}

export interface WorkerWithdrawalReturnTransaction {
  id: string;
  type: 'withdrawal' | 'deduction' | 'return' | 'deduction_payment' | 'settlement';
  amount: Money;
  occurred_at: string;
  notes?: string | null;
  created_by_name?: string;
  editable?: boolean;
  deletable?: boolean;
}

export interface WorkerWithdrawalReturnLedger {
  worker_id: string;
  worker_name: string;
  total_withdrawals: Money;
  total_deductions: Money;
  total_returns: Money;
  total_deduction_payments: Money;
  total_settlements: Money;
  outstanding_balance: Money;
  transactions: WorkerWithdrawalReturnTransaction[];
}

export interface PayrollEmployee {
  employee_id: string;
  employee_name: string;
  is_active: boolean;
  salary: Money;
  total_withdrawals: Money;
  total_deductions: Money;
  remaining_salary: Money;
  salary_configured: boolean;
}

export interface SalaryDeduction {
  id: string;
  employee_id: string;
  employee_name: string;
  amount: Money;
  deducted_at: string;
  notes?: string | null;
  created_by_name?: string;
  created_at?: string;
  updated_at?: string;
}

export interface SalaryWithdrawal {
  id: string;
  employee_id: string;
  employee_name: string;
  amount: Money;
  withdrawn_at: string;
  notes?: string | null;
  created_by_name?: string;
  created_at?: string;
  updated_at?: string;
}

export interface PayrollSummary {
  month: string;
  employees: PayrollEmployee[];
  total_salary: Money;
  total_withdrawals: Money;
  total_deductions: Money;
  total_remaining: Money;
}

export type ExpenseAllocation = 'business_only' | 'workers_only' | 'shared';

export interface ExpenseRecord {
  id: string;
  description: string;
  amount: Money;
  spent_at?: string;
  date?: string;
  notes?: string | null;
  allocation: ExpenseAllocation;
  business_percentage?: number | string | null;
  workers_percentage?: number | string | null;
  business_amount?: Money;
  workers_amount?: Money;
  created_by_name?: string;
  created_at?: string;
  allocations?: Array<{ worker_id: string; worker_name: string; amount: Money; created_at?: string }>;
}

export interface OperationalReport {
  cars_washed?: number;
  washes?: Wash[];
  workers?: Array<{
    worker_id: string;
    worker_name: string;
    cars_washed: number;
  }>;
}

export interface FinancialReport extends FinanceOverview {
  worker_performance?: Array<{
    worker_id: string;
    worker_name: string;
    cars_washed: number;
    commissions?: Money;
    deductions?: Money;
    payable?: Money;
  }>;
  expenses?: ExpenseRecord[];
  showroom_payments?: PaymentRecord[];
}

export interface AppSettings {
  business_name?: string;
  currency?: string;
  default_worker_commission_percentage?: number | string;
  last_backup_at?: string | null;
}

export interface Role {
  id: string;
  key: string;
  name: string;
  permissions: string[];
}

export interface AuditLog {
  id: string;
  action: string;
  description?: string;
  affected_record?: string;
  created_at: string;
  user_name?: string;
}

export interface BackupRecord {
  id: string;
  file_name?: string;
  created_at: string;
  size_bytes?: number;
  path?: string;
  download_url?: string;
}

export interface DateRange {
  from: string;
  to: string;
}
