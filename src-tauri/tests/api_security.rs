use alkaheli_car_wash_erp_lib::{api::build_router, create_state};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
use tower::ServiceExt;
use uuid::Uuid;

const MANAGER_PASSWORD: &str = "ManagerPass123!";
const EMPLOYEE_PASSWORD: &str = "EmployeePass123!";
const ALL_TIME_RANGE: &str = "from=0000-01-01T00:00:00Z&to=9999-12-31T23:59:59Z";

fn all_time_endpoint(path: &str) -> String {
    format!("{path}?{ALL_TIME_RANGE}")
}

fn business_today_key() -> String {
    (Utc::now() + Duration::hours(2))
        .format("%Y-%m-%d")
        .to_string()
}

struct TestApp {
    router: Router,
    data_dir: PathBuf,
}

#[tokio::test]
async fn wash_type_is_created_listed_edited_and_persisted_after_restart() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل اختبار نوع الغسيل").await;
    let occurred_at = "2026-09-01T10:00:00Z";

    let (status, created) = request_json(
        &test_app.router,
        Method::POST,
        "/api/washes",
        Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota",
            "vehicleModel":"Camry",
            "washType":"Inside Only",
            "price":"50",
            "workerId":worker_id,
            "paymentType":"cash",
            "occurredAt":occurred_at,
            "clientRequestId":"wash-type-persistence-test"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let wash_id = created["data"]["wash"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["data"]["wash"]["washType"], "Inside Only");
    assert_eq!(created["data"]["wash"]["priceMilli"], 50_000);
    assert_eq!(created["data"]["wash"]["commissionMilli"], 25_000);

    let list_endpoint = "/api/washes?from=2026-09-01T00:00:00Z&to=2026-09-01T23:59:59Z";
    let (status, listed) = request_json(
        &test_app.router,
        Method::GET,
        list_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["data"]["items"][0]["vehicleMake"], "Toyota");
    assert_eq!(listed["data"]["items"][0]["vehicleModel"], "Camry");
    assert_eq!(listed["data"]["items"][0]["washType"], "Inside Only");

    let paid_status_endpoint = all_time_endpoint(&format!("/api/washes/{wash_id}/paid"));
    let (status, paid) = request_json(
        &test_app.router,
        Method::PATCH,
        &paid_status_endpoint,
        Some(&manager_token),
        Some(json!({"isPaid":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paid["data"]["wash"]["washType"], "Inside Only");

    let paid_cars_endpoint = all_time_endpoint("/api/paid-cars");
    let (status, completed) = request_json(
        &test_app.router,
        Method::GET,
        &paid_cars_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(completed["data"]["items"][0]["id"], wash_id);
    assert_eq!(completed["data"]["items"][0]["washType"], "Inside Only");

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &paid_status_endpoint,
        Some(&manager_token),
        Some(json!({"isPaid":false})),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (status, overnight_marked) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_id}/overnight"),
        Some(&manager_token),
        Some(json!({"isOvernight":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overnight_marked["data"]["wash"]["washType"], "Inside Only");

    let overnight_endpoint = all_time_endpoint("/api/overnight-cars");
    let (status, overnight) = request_json(
        &test_app.router,
        Method::GET,
        &overnight_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overnight["data"]["items"][0]["wash"]["id"], wash_id);
    assert_eq!(overnight["data"]["items"][0]["wash"]["washType"], "Inside Only");

    let (status, updated) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_id}"),
        Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota",
            "vehicleModel":"Camry",
            "washType":"Inside Only Updated",
            "price":"50",
            "workerId":worker_id,
            "paymentType":"cash",
            "occurredAt":occurred_at
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["wash"]["washType"], "Inside Only Updated");
    assert_eq!(updated["data"]["wash"]["commissionMilli"], 25_000);

    let (_, overnight_after_edit) = request_json(
        &test_app.router,
        Method::GET,
        &overnight_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(overnight_after_edit["data"]["items"][0]["wash"]["washType"], "Inside Only Updated");

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &paid_status_endpoint,
        Some(&manager_token),
        Some(json!({"isPaid":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, completed_after_edit) = request_json(
        &test_app.router,
        Method::GET,
        &paid_cars_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(completed_after_edit["data"]["items"][0]["washType"], "Inside Only Updated");
    let (_, inactive_overnight_after_paid) = request_json(
        &test_app.router,
        Method::GET,
        &overnight_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert!(inactive_overnight_after_paid["data"]["items"].as_array().unwrap().is_empty());

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened = build_router(create_state(data_dir.clone()).expect("wash type database should reopen"));
    let reopened_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, completed_after_restart) = request_json(
        &reopened,
        Method::GET,
        &paid_cars_endpoint,
        Some(&reopened_token),
        None,
    ).await;
    assert_eq!(completed_after_restart["data"]["items"][0]["washType"], "Inside Only Updated");
    let (_, overnight_after_restart) = request_json(
        &reopened,
        Method::GET,
        &overnight_endpoint,
        Some(&reopened_token),
        None,
    ).await;
    assert!(overnight_after_restart["data"]["items"].as_array().unwrap().is_empty());
    drop(reopened);

    let connection = Connection::open(data_dir.join("carwash.db")).unwrap();
    let stored: Option<String> = connection.query_row(
        "SELECT wash_type FROM wash_operations WHERE id=?1",
        [&wash_id],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(stored.as_deref(), Some("Inside Only Updated"));
    assert_eq!(connection.query_row(
        "SELECT COUNT(*) FROM overnight_cars WHERE wash_id=?1",
        [&wash_id],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 1);
    assert_eq!(connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('overnight_cars') WHERE name='wash_type'",
        [],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 0);
    assert_eq!(connection.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version=19",
        [],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 1);
    drop(connection);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn workers_and_payroll_employees_are_independent_and_persist_after_restart() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل مستقل عن المرتبات").await;

    let (_, empty_payroll) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert!(empty_payroll["data"]["employees"].as_array().unwrap().is_empty(), "creating a worker must not create or expose a payroll employee");
    let (_, workers) = request_json(&test_app.router, Method::GET, "/api/workers", Some(&manager_token), None).await;
    assert!(workers["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == worker_id));

    create_cash_wash(&test_app.router, &manager_token, &worker_id, "50").await;
    let (_, worker_financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")), Some(&manager_token), None).await;
    assert_eq!(worker_financial["data"]["grossCommissionMilli"], 25_000);

    let (status, created) = request_json(
        &test_app.router,
        Method::POST,
        "/api/payroll/employees",
        Some(&manager_token),
        Some(json!({"fullName":"موظف مرتبات يدوي","month":"2026-08","salary":"900"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_id = created["data"]["employee"]["id"].as_str().unwrap().to_owned();

    let (_, payroll) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    let employee = payroll["data"]["employees"].as_array().unwrap().iter()
        .find(|item| item["employee"]["id"] == employee_id).unwrap();
    assert_eq!(employee["employee"]["fullName"], "موظف مرتبات يدوي");
    assert_eq!(employee["salaryMilli"], 900_000);
    assert_eq!(employee["totalWithdrawalsMilli"], 0);
    assert_eq!(employee["remainingSalaryMilli"], 900_000);
    assert!(!payroll["data"]["employees"].as_array().unwrap().iter().any(|item| item["employee"]["id"] == worker_id));
    let (_, workers_after_employee) = request_json(&test_app.router, Method::GET, "/api/workers", Some(&manager_token), None).await;
    assert!(!workers_after_employee["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == employee_id), "creating a payroll employee must not create a worker");

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/workers/{worker_id}"), Some(&manager_token),
        Some(json!({"fullName":"عامل مستقل معدل","phone":"0920000000","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, payroll_after_worker_edit) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(payroll_after_worker_edit["data"]["employees"][0]["employee"]["fullName"], "موظف مرتبات يدوي");

    let (status, _) = request_json(
        &test_app.router, Method::PUT, &format!("/api/payroll/employees/{employee_id}/salary"), Some(&manager_token),
        Some(json!({"month":"2026-08","salary":"950"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, worker_after_salary_edit) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert_eq!(worker_after_salary_edit["data"]["worker"]["fullName"], "عامل مستقل معدل");

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/payroll/withdrawals",
        Some(&manager_token),
        Some(json!({"employeeId":employee_id,"amount":"125","withdrawnAt":"2026-08-15T12:00:00Z","notes":"سلفة"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, payroll_after_withdrawal) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    let employee = payroll_after_withdrawal["data"]["employees"].as_array().unwrap().iter()
        .find(|item| item["employee"]["id"] == employee_id).unwrap();
    assert_eq!(employee["totalWithdrawalsMilli"], 125_000);
    assert_eq!(employee["remainingSalaryMilli"], 825_000);

    let (status, created_deduction) = request_json(
        &test_app.router,
        Method::POST,
        "/api/payroll/deductions",
        Some(&manager_token),
        Some(json!({"employeeId":employee_id,"amount":"75","deductedAt":"2026-08-18T12:00:00Z","notes":"تأخير"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let deduction_id = created_deduction["data"]["id"].as_str().unwrap().to_owned();
    let (status, history) = request_json(&test_app.router, Method::GET, "/api/payroll/deductions?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["data"]["items"][0]["id"], deduction_id);
    assert_eq!(history["data"]["items"][0]["notes"], "تأخير");
    let (_, payroll_after_deduction) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    let employee = payroll_after_deduction["data"]["employees"].as_array().unwrap().iter()
        .find(|item| item["employee"]["id"] == employee_id).unwrap();
    assert_eq!(employee["totalWithdrawalsMilli"], 125_000);
    assert_eq!(employee["totalDeductionsMilli"], 75_000);
    assert_eq!(employee["remainingSalaryMilli"], 750_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/payroll/deductions/{deduction_id}"),
        Some(&manager_token),
        Some(json!({"employeeId":employee_id,"amount":"90","deductedAt":"2026-08-20T12:00:00Z","notes":"تأخير معدل"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, payroll_after_edit) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    let employee = payroll_after_edit["data"]["employees"].as_array().unwrap().iter()
        .find(|item| item["employee"]["id"] == employee_id).unwrap();
    assert_eq!(employee["totalDeductionsMilli"], 90_000);
    assert_eq!(employee["remainingSalaryMilli"], 735_000);

    let (status, deleted_worker) = request_json(&test_app.router, Method::DELETE, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted_worker["data"]["worker"]["id"], worker_id);
    assert_eq!(deleted_worker["data"]["archived"], true);
    let (_, workers_after_worker_delete) = request_json(&test_app.router, Method::GET, "/api/workers", Some(&manager_token), None).await;
    assert!(!workers_after_worker_delete["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == worker_id));
    let (_, payroll_after_worker_delete) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert!(payroll_after_worker_delete["data"]["employees"].as_array().unwrap().iter().any(|item| item["employee"]["id"] == employee_id));

    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/payroll/employees/{employee_id}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, worker_after_employee_delete) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert_eq!(worker_after_employee_delete["data"]["worker"]["fullName"], "عامل مستقل معدل");
    let (_, worker_financial_after_deletes) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")), Some(&manager_token), None).await;
    assert_eq!(worker_financial_after_deletes["data"]["grossCommissionMilli"], 25_000);

    let (_, users) = request_json(&test_app.router, Method::GET, "/api/users", Some(&manager_token), None).await;
    assert!(!users["data"]["items"].as_array().unwrap().iter().any(|user| user["fullName"] == "موظف مرتبات يدوي"));

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened = build_router(create_state(data_dir.clone()).expect("separated payroll database should reopen"));
    let reopened_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, payroll_after_restart) = request_json(&reopened, Method::GET, "/api/payroll?month=2026-08", Some(&reopened_token), None).await;
    assert!(payroll_after_restart["data"]["employees"].as_array().unwrap().is_empty());
    let (_, worker_after_restart) = request_json(&reopened, Method::GET, &format!("/api/workers/{worker_id}"), Some(&reopened_token), None).await;
    assert_eq!(worker_after_restart["data"]["worker"]["fullName"], "عامل مستقل معدل");
    drop(reopened);

    let connection = Connection::open(data_dir.join("carwash.db")).unwrap();
    assert_eq!(connection.query_row("SELECT is_active FROM workers WHERE id=?1", [worker_id], |row| row.get::<_,i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM wash_operations", [], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM payroll_employees WHERE id=?1", [employee_id.clone()], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM payroll_salary_rates WHERE employee_id=?1", [employee_id.clone()], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM salary_withdrawals WHERE employee_id=?1", [employee_id.clone()], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM salary_deductions WHERE employee_id=?1", [employee_id], |row| row.get::<_,i64>(0)).unwrap(), 1);
    drop(connection);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn showroom_payments_reduce_debt_and_edit_delete_persist_without_touching_washes() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل دفعات المعرض").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض دفعات الاختبار","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap().to_owned();
    for (price, plate) in [("600", "600 د ي"), ("400", "400 د ي")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
            Some(json!({
                "vehicleMake":"Toyota","vehicleModel":"Corolla","licensePlate":plate,
                "price":price,"workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,
                "showroomPaymentMethod":"bank","occurredAt":"2026-08-12T10:00:00Z",
                "clientRequestId":Uuid::new_v4().to_string()
            })),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    let detail_url = format!("/api/showroom-debts/{showroom_id}?from=2026-08-01T00:00:00Z&to=2026-08-31T23:59:59Z");
    let (_, before_payment) = request_json(&test_app.router, Method::GET, &detail_url, Some(&manager_token), None).await;
    assert_eq!(before_payment["data"]["totalChargesMilli"], 1_000_000);
    assert_eq!(before_payment["data"]["totalPaymentsMilli"], 0);
    assert_eq!(before_payment["data"]["totalOutstandingMilli"], 1_000_000);
    assert_eq!(before_payment["data"]["operations"].as_array().unwrap().len(), 2);

    let (status, created_payment) = request_json(
        &test_app.router, Method::POST, "/api/showroom-payments", Some(&manager_token),
        Some(json!({"showroomId":showroom_id,"amount":"300","paidAt":"2026-08-15T12:00:00Z","notes":"دفعة أولى"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let payment_id = created_payment["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created_payment["data"]["showroom"]["id"], showroom_id);
    let (_, after_create) = request_json(&test_app.router, Method::GET, &detail_url, Some(&manager_token), None).await;
    assert_eq!(after_create["data"]["totalOutstandingMilli"], 700_000);
    assert_eq!(after_create["data"]["payments"].as_array().unwrap().len(), 1);

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/showroom-payments/{payment_id}"), Some(&manager_token),
        Some(json!({"showroomId":showroom_id,"amount":"200","paidAt":"2026-08-15T12:00:00Z","notes":"دفعة معدلة"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_edit) = request_json(&test_app.router, Method::GET, &detail_url, Some(&manager_token), None).await;
    assert_eq!(after_edit["data"]["totalOutstandingMilli"], 800_000);
    assert_eq!(after_edit["data"]["payments"][0]["amountMilli"], 200_000);

    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/showroom-payments/{payment_id}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_delete) = request_json(&test_app.router, Method::GET, &detail_url, Some(&manager_token), None).await;
    assert_eq!(after_delete["data"]["totalOutstandingMilli"], 1_000_000);
    assert!(after_delete["data"]["payments"].as_array().unwrap().is_empty());
    assert_eq!(after_delete["data"]["operations"].as_array().unwrap().len(), 2);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened_router = build_router(create_state(data_dir.clone()).expect("database should reopen"));
    let reopened_token = login(&reopened_router, "manager.test", MANAGER_PASSWORD).await;
    let (_, persisted) = request_json(&reopened_router, Method::GET, &detail_url, Some(&reopened_token), None).await;
    assert_eq!(persisted["data"]["totalChargesMilli"], 1_000_000);
    assert_eq!(persisted["data"]["totalPaymentsMilli"], 0);
    assert_eq!(persisted["data"]["totalOutstandingMilli"], 1_000_000);
    assert_eq!(persisted["data"]["operations"].as_array().unwrap().len(), 2);
    drop(reopened_router);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn showroom_debt_report_range_filters_operation_dates_and_recalculates_totals() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل تقرير فترة المعرض").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض اختبار فترة التقرير","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap().to_owned();

    for (occurred_at, price, plate) in [
        ("2026-08-10T09:00:00Z", "100", "أغسطس 10"),
        ("2026-08-20T18:30:00Z", "200", "أغسطس 20"),
        ("2026-08-31T22:00:00Z", "300", "سبتمبر 01"),
    ] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
            Some(json!({
                "vehicleMake":"Toyota","vehicleModel":"Test","licensePlate":plate,
                "price":price,"workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,
                "showroomPaymentMethod":"cash","occurredAt":occurred_at,
                "clientRequestId":Uuid::new_v4().to_string()
            })),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, _) = request_json(
        &test_app.router, Method::POST, "/api/showroom-payments", Some(&manager_token),
        Some(json!({"showroomId":showroom_id,"amount":"50","paidAt":"2026-08-15T12:00:00Z","notes":"دفعة أغسطس"})),
    ).await;
    assert_eq!(status, StatusCode::OK);

    // Africa/Tripoli is UTC+02:00, so this is the inclusive local range
    // 2026-08-01 00:00:00 through 2026-08-31 23:59:59.999.
    let august_url = format!(
        "/api/showroom-debts/{showroom_id}?from=2026-07-31T22:00:00.000Z&to=2026-08-31T21:59:59.999Z"
    );
    let (status, august) = request_json(
        &test_app.router, Method::GET, &august_url, Some(&manager_token), None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(august["data"]["outstandingWashCount"], 2);
    assert_eq!(august["data"]["totalChargesMilli"], 300_000);
    assert_eq!(august["data"]["totalPaymentsMilli"], 50_000);
    assert_eq!(august["data"]["totalOutstandingMilli"], 250_000);
    let operations = august["data"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 2);
    assert!(operations.iter().any(|operation| operation["licensePlate"] == "أغسطس 10"));
    assert!(operations.iter().any(|operation| operation["licensePlate"] == "أغسطس 20"));
    assert!(!operations.iter().any(|operation| operation["licensePlate"] == "سبتمبر 01"));

    let all_time_url = format!(
        "/api/showroom-debts/{showroom_id}?from=0000-01-01T00:00:00Z&to=9999-12-31T23:59:59Z"
    );
    let (_, unchanged) = request_json(
        &test_app.router, Method::GET, &all_time_url, Some(&manager_token), None,
    ).await;
    assert_eq!(unchanged["data"]["operations"].as_array().unwrap().len(), 3);
    assert_eq!(unchanged["data"]["payments"].as_array().unwrap().len(), 1);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn payroll_tracks_effective_month_salaries_and_recalculates_withdrawal_history() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let (status, created_employee) = request_json(
        &test_app.router, Method::POST, "/api/payroll/employees", Some(&manager_token),
        Some(json!({"fullName":"موظف المرتبات","month":"2026-08","salary":"1000"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_id = created_employee["data"]["employee"]["id"].as_str().unwrap().to_owned();

    let (status, _) = request_json(
        &test_app.router,
        Method::GET,
        "/api/payroll?month=2026-08",
        None,
        None,
    ).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, july) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-07", Some(&manager_token), None).await;
    let (_, august) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(july["data"]["employees"][0]["salaryMilli"], 0);
    assert_eq!(august["data"]["employees"][0]["salaryMilli"], 1_000_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        &format!("/api/payroll/employees/{employee_id}/salary"),
        Some(&manager_token),
        Some(json!({"month":"2026-09","salary":"1200"})),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (status, created) = request_json(
        &test_app.router,
        Method::POST,
        "/api/payroll/withdrawals",
        Some(&manager_token),
        Some(json!({"employeeId":employee_id,"amount":"200","withdrawnAt":"2026-08-15T12:00:00Z","notes":"سلفة"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawal_id = created["data"]["id"].as_str().unwrap().to_owned();

    let (_, august) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(august["data"]["totalSalaryMilli"], 1_000_000);
    assert_eq!(august["data"]["totalWithdrawalsMilli"], 200_000);
    assert_eq!(august["data"]["totalRemainingMilli"], 800_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/payroll/withdrawals/{withdrawal_id}"),
        Some(&manager_token),
        Some(json!({"employeeId":employee_id,"amount":"250","withdrawnAt":"2026-09-10T12:00:00Z","notes":"سلفة معدلة"})),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (_, august_after_edit) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    let (_, september) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-09", Some(&manager_token), None).await;
    assert_eq!(august_after_edit["data"]["totalRemainingMilli"], 1_000_000);
    assert_eq!(september["data"]["totalSalaryMilli"], 1_200_000);
    assert_eq!(september["data"]["totalWithdrawalsMilli"], 250_000);
    assert_eq!(september["data"]["totalRemainingMilli"], 950_000);

    let (status, history) = request_json(&test_app.router, Method::GET, "/api/payroll/withdrawals?month=2026-09", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["data"]["items"][0]["notes"], "سلفة معدلة");

    let (status, _) = request_json(
        &test_app.router,
        Method::DELETE,
        &format!("/api/payroll/withdrawals/{withdrawal_id}"),
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, september_after_delete) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-09", Some(&manager_token), None).await;
    assert_eq!(september_after_delete["data"]["totalWithdrawalsMilli"], 0);
    assert_eq!(september_after_delete["data"]["totalRemainingMilli"], 1_200_000);

    let (_, august_history) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(august_history["data"]["totalSalaryMilli"], 1_000_000, "later salary changes must not rewrite previous months");

    let (status, _) = request_json(
        &test_app.router,
        Method::DELETE,
        &format!("/api/payroll/employees/{employee_id}"),
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, active_payroll) = request_json(&test_app.router, Method::GET, "/api/payroll?month=2026-08", Some(&manager_token), None).await;
    assert_eq!(active_payroll["data"]["employees"].as_array().unwrap().len(), 0);

    test_app.cleanup();
}

#[tokio::test]
async fn obsolete_worker_payments_are_migrated_out_and_never_reduce_worker_balances() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل تنظيف الدفعات القديمة").await;
    create_cash_wash(&test_app.router, &manager_token, &worker_id, "50").await;

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let database_path = data_dir.join("carwash.db");
    let connection = Connection::open(&database_path).unwrap();
    let manager_id: String = connection.query_row(
        "SELECT id FROM users WHERE username_norm='manager.test'",
        [],
        |row| row.get(0),
    ).unwrap();
    let payment_id = Uuid::new_v4().to_string();
    let transaction_id = Uuid::new_v4().to_string();
    connection.execute("DELETE FROM schema_migrations WHERE version=12", []).unwrap();
    connection.execute(
        "INSERT INTO worker_payments(id,worker_id,amount_milli,paid_at,notes,created_by,created_at)
         VALUES(?1,?2,200000,'2026-08-29T12:00:00Z','دفعة قديمة',?3,'2026-08-29T12:00:00Z')",
        params![payment_id, worker_id, manager_id],
    ).unwrap();
    connection.execute(
        "INSERT INTO financial_transactions(id,source_type,source_id,occurred_at,created_by,created_at)
         VALUES(?1,'worker_payment',?2,'2026-08-29T12:00:00Z',?3,'2026-08-29T12:00:00Z')",
        params![transaction_id, payment_id, manager_id],
    ).unwrap();
    connection.execute(
        "INSERT INTO ledger_entries(id,transaction_id,account_code,entry_side,amount_milli,related_worker_id,created_at)
         VALUES(?1,?2,'WORKER_PAYABLE','debit',200000,?3,'2026-08-29T12:00:00Z')",
        params![Uuid::new_v4().to_string(), transaction_id, worker_id],
    ).unwrap();
    connection.execute(
        "INSERT INTO audit_logs(id,user_id,action,entity_type,entity_id,description,created_at)
         VALUES(?1,?2,'WORKER_PAYMENT_RECORDED','worker_payment',?3,'دفعة عامل قديمة','2026-08-29T12:00:00Z')",
        params![Uuid::new_v4().to_string(), manager_id, payment_id],
    ).unwrap();
    drop(connection);

    let reopened_router = build_router(create_state(data_dir.clone()).expect("cleanup migration should reopen the database"));
    let reopened_token = login(&reopened_router, "manager.test", MANAGER_PASSWORD).await;
    let (status, worker_financial) = request_json(
        &reopened_router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")), Some(&reopened_token), None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(worker_financial["data"]["grossCommissionMilli"], 25_000);
    assert_eq!(worker_financial["data"]["deductionsMilli"], 0);
    assert_eq!(worker_financial["data"]["paidMilli"], 0);
    assert_eq!(worker_financial["data"]["remainingMilli"], 25_000);
    let (_, finance) = request_json(&reopened_router, Method::GET, &all_time_endpoint("/api/finance/overview"), Some(&reopened_token), None).await;
    assert_eq!(finance["data"]["outstandingWorkerBalancesMilli"], 25_000);
    assert!(finance["data"].get("workerPaymentsMilli").is_none());
    let (_, dashboard) = request_json(&reopened_router, Method::GET, "/api/dashboard?date=2026-08-29", Some(&reopened_token), None).await;
    assert_eq!(dashboard["data"]["financial"]["workerPayable"], 25_000);
    let removed_endpoint_status = reopened_router.clone().oneshot(
        Request::builder()
            .method(Method::GET)
            .uri("/api/worker-payments")
            .header(header::AUTHORIZATION, format!("Bearer {reopened_token}"))
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap().status();
    assert_eq!(removed_endpoint_status, StatusCode::NOT_FOUND);
    drop(reopened_router);

    let connection = Connection::open(&database_path).unwrap();
    for sql in [
        "SELECT COUNT(*) FROM worker_payments",
        "SELECT COUNT(*) FROM financial_transactions WHERE source_type='worker_payment'",
        "SELECT COUNT(*) FROM audit_logs WHERE entity_type='worker_payment' OR action='WORKER_PAYMENT_RECORDED'",
    ] {
        let count: i64 = connection.query_row(sql, [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "obsolete worker-payment data must be fully removed");
    }
    drop(connection);
    let _ = fs::remove_dir_all(data_dir);
}

impl TestApp {
    fn new() -> Self {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-car-wash-api-tests")
            .join(Uuid::new_v4().to_string());
        let state = create_state(data_dir.clone()).expect("test database should be created");

        Self {
            router: build_router(state),
            data_dir,
        }
    }

    fn cleanup(self) {
        let data_dir = self.data_dir.clone();
        drop(self);
        let _ = fs::remove_dir_all(data_dir);
    }
}

async fn request_json(
    router: &Router,
    method: Method,
    uri: &str,
    bearer_token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer_token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    let body = match body {
        Some(payload) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(payload.to_string())
        }
        None => Body::empty(),
    };

    let response = router
        .clone()
        .oneshot(request.body(body).expect("request should be valid"))
        .await
        .expect("router should produce a response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let payload = serde_json::from_slice(&bytes).expect("API should return JSON");

    (status, payload)
}

async fn request_bytes(
    router: &Router,
    method: Method,
    uri: &str,
    bearer_token: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer_token}"));
    if let Some(value) = content_type { request = request.header(header::CONTENT_TYPE, value); }
    let response = router.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
    (status, headers, bytes)
}

async fn upload_test_image(router: &Router, token: &str, user_id: &str, payload: &[u8]) -> StatusCode {
    let boundary = "profile-picture-test-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"avatar.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
    ).into_bytes();
    body.extend_from_slice(payload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/api/users/{user_id}/profile-picture"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    router.clone().oneshot(request).await.unwrap().status()
}

async fn bootstrap_manager(router: &Router) -> String {
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/setup/initial-manager",
        None,
        Some(json!({
            "fullName": "مدير الاختبار",
            "username": "manager.test",
            "password": MANAGER_PASSWORD,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    payload["data"]["token"]
        .as_str()
        .expect("initial manager should receive a session token")
        .to_owned()
}

async fn create_worker(router: &Router, manager_token: &str, full_name: &str) -> String {
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/workers",
        Some(manager_token),
        Some(json!({
            "fullName": full_name,
            "phone": "0910000000",
            "isActive": true,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    payload["data"]["id"]
        .as_str()
        .expect("worker creation should return an ID")
        .to_owned()
}

async fn create_cash_wash(router: &Router, token: &str, worker_id: &str, price: &str) -> String {
    let request_id = Uuid::new_v4().to_string();
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/washes",
        Some(token),
        Some(json!({
            "vehicleMake": "Toyota",
            "vehicleModel": "Camry",
            "manufactureYear": 2024,
            "licensePlate": "1234 أ ب",
            "price": price,
            "workerId": worker_id,
            "paymentType": "cash",
            "occurredAt": "2026-08-29T10:00:00Z",
            "clientRequestId": request_id,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["duplicate"], false);
    assert_eq!(payload["data"]["wash"]["worker"]["id"], worker_id);
    assert!(!payload["data"]["wash"]["worker"]["fullName"].as_str().unwrap_or_default().is_empty());
    assert!(payload["data"]["wash"]["commissionMilli"].as_i64().is_some());
    payload["data"]["id"]
        .as_str()
        .expect("wash creation should return an ID")
        .to_owned()
}

async fn create_cash_wash_at(router: &Router, token: &str, worker_id: &str, price: &str, occurred_at: &str) -> String {
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/washes",
        Some(token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Camry","manufactureYear":2024,"licensePlate":"1234 أ ب","price":price,"workerId":worker_id,"paymentType":"cash","occurredAt":occurred_at,"clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["wash"]["worker"]["id"], worker_id);
    assert!(!payload["data"]["wash"]["worker"]["fullName"].as_str().unwrap_or_default().is_empty());
    payload["data"]["id"].as_str().unwrap().to_owned()
}

async fn create_employee_cash_wash(router: &Router, token: &str, worker_id: &str, plate: &str) -> String {
    let occurred_at = format!("{}T10:00:00Z", business_today_key());
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/washes",
        Some(token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Corolla","manufactureYear":2024,"licensePlate":plate,"price":"40","workerId":worker_id,"paymentType":"cash","occurredAt":occurred_at,"clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["wash"]["worker"]["id"], worker_id);
    assert!(!payload["data"]["wash"]["worker"]["fullName"].as_str().unwrap_or_default().is_empty());
    payload["data"]["id"].as_str().unwrap().to_owned()
}

async fn login(router: &Router, username: &str, password: &str) -> String {
    let (status, payload) = request_json(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    payload["data"]["token"]
        .as_str()
        .expect("login should return a session token")
        .to_owned()
}

fn assert_no_sensitive_financial_keys(value: &Value) {
    const FORBIDDEN: &[&str] = &[
        "commissionBps",
        "commissionBpsOverride",
        "commissionMilli",
        "businessShareMilli",
        "grossCommissionMilli",
        "deductionsMilli",
        "netEarningsMilli",
        "paidMilli",
        "remainingMilli",
        "financial",
        "priceMilli",
    ];

    match value {
        Value::Object(object) => {
            for key in FORBIDDEN {
                assert!(
                    !object.contains_key(*key),
                    "operational response exposed manager-only key `{key}`: {value}"
                );
            }
            for child in object.values() {
                assert_no_sensitive_financial_keys(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_no_sensitive_financial_keys(child);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn initial_manager_setup_and_login_work() {
    let test_app = TestApp::new();

    let (status, payload) = request_json(
        &test_app.router,
        Method::GET,
        "/api/setup/status",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["needsSetup"], true);

    let initial_token = bootstrap_manager(&test_app.router).await;
    assert!(!initial_token.is_empty());

    let (status, payload) = request_json(
        &test_app.router,
        Method::GET,
        "/api/setup/status",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["needsSetup"], false);

    let login_token = login(&test_app.router, "manager.test", MANAGER_PASSWORD).await;
    let (status, payload) = request_json(
        &test_app.router,
        Method::GET,
        "/api/auth/me",
        Some(&login_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["roleCode"], "manager");
    assert_eq!(payload["data"]["username"], "manager.test");

    test_app.cleanup();
}

#[tokio::test]
async fn profile_pictures_are_isolated_persistent_and_removable() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let mut employee_ids = Vec::new();
    let mut employee_tokens = Vec::new();
    for (index, username) in ["picture.employee.a", "picture.employee.b"].iter().enumerate() {
        let (status, created) = request_json(
            &test_app.router, Method::POST, "/api/users", Some(&manager_token),
            Some(json!({"fullName":format!("موظف الصورة {index}"),"username":username,"password":EMPLOYEE_PASSWORD,"roleCode":"employee","isActive":true})),
        ).await;
        assert_eq!(status, StatusCode::OK);
        employee_ids.push(created["data"]["id"].as_str().unwrap().to_owned());
        employee_tokens.push(login(&test_app.router, username, EMPLOYEE_PASSWORD).await);
    }
    let jpeg_a = [0xff, 0xd8, 0xff, 0x01, 0x02, 0x03];
    let jpeg_b = [0xff, 0xd8, 0xff, 0x04, 0x05, 0x06];
    assert_eq!(upload_test_image(&test_app.router, &employee_tokens[0], &employee_ids[0], &jpeg_a).await, StatusCode::OK);
    assert_eq!(upload_test_image(&test_app.router, &employee_tokens[1], &employee_ids[1], &jpeg_b).await, StatusCode::OK);
    let (status_a, _, bytes_a) = request_bytes(&test_app.router, Method::GET, &format!("/api/users/{}/profile-picture", employee_ids[0]), &employee_tokens[0], None, Vec::new()).await;
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(bytes_a, jpeg_a);
    let (status_cross, _, _) = request_bytes(&test_app.router, Method::GET, &format!("/api/users/{}/profile-picture", employee_ids[1]), &employee_tokens[0], None, Vec::new()).await;
    assert_eq!(status_cross, StatusCode::FORBIDDEN);
    let (status_manager, _, bytes_manager) = request_bytes(&test_app.router, Method::GET, &format!("/api/users/{}/profile-picture", employee_ids[1]), &manager_token, None, Vec::new()).await;
    assert_eq!(status_manager, StatusCode::OK);
    assert_eq!(bytes_manager, jpeg_b);
    assert!(test_app.data_dir.join("profile-pictures").join(format!("{}.img", employee_ids[0])).is_file());
    let (status_remove, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/users/{}/profile-picture", employee_ids[0]), Some(&employee_tokens[0]), None).await;
    assert_eq!(status_remove, StatusCode::OK);
    let (status_missing, _, _) = request_bytes(&test_app.router, Method::GET, &format!("/api/users/{}/profile-picture", employee_ids[0]), &employee_tokens[0], None, Vec::new()).await;
    assert_eq!(status_missing, StatusCode::NOT_FOUND);
    test_app.cleanup();
}

#[tokio::test]
async fn manager_backup_and_restore_preserve_a_verified_snapshot() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;

    let (status, backup) = request_json(
        &test_app.router,
        Method::POST,
        "/api/backups",
        Some(&manager_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let path = backup["data"]["path"]
        .as_str()
        .expect("backup path should be returned")
        .to_owned();
    assert!(std::path::Path::new(&path).is_file());
    let backup_id = backup["data"]["id"].as_str().unwrap().to_owned();

    let (status, history) = request_json(&test_app.router, Method::GET, "/api/backups", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(history["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == backup_id));

    let (status, headers, downloaded) = request_bytes(
        &test_app.router, Method::GET, &format!("/api/backups/{backup_id}/download"), &manager_token, None, Vec::new(),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "application/octet-stream");
    assert!(headers.get(header::CONTENT_DISPOSITION).unwrap().to_str().unwrap().contains("attachment"));
    let downloaded_path = test_app.data_dir.join("downloaded-backup.db");
    fs::write(&downloaded_path, &downloaded).unwrap();
    assert!(downloaded.len() > 1000);

    create_worker(&test_app.router, &manager_token, "عامل بعد النسخة").await;
    let boundary = format!("alkaheli-test-{}", Uuid::new_v4());
    let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"backup\"; filename=\"downloaded-backup.db\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    multipart.extend_from_slice(&downloaded);
    multipart.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"confirmation\"\r\n\r\nRESTORE\r\n--{boundary}--\r\n").as_bytes());
    let (status, _, restored_bytes) = request_bytes(
        &test_app.router, Method::POST, "/api/backups/restore-upload", &manager_token,
        Some(&format!("multipart/form-data; boundary={boundary}")), multipart,
    ).await;
    let restored: Value = serde_json::from_slice(&restored_bytes).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["data"]["restored"], true);
    assert!(restored["data"]["emergencyBackupPath"].as_str().is_some());

    let (status, _) = request_json(
        &test_app.router,
        Method::GET,
        "/api/dashboard",
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "restore must invalidate the old session"
    );

    let new_token = login(&test_app.router, "manager.test", MANAGER_PASSWORD).await;
    let (status, workers) = request_json(
        &test_app.router,
        Method::GET,
        "/api/workers",
        Some(&new_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        workers["data"]["items"]
            .as_array()
            .expect("worker list")
            .len(),
        0
    );

    let (status, history) = request_json(&test_app.router, Method::GET, "/api/backups", Some(&new_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(history["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == backup_id));
    let (status, _, downloaded_again) = request_bytes(
        &test_app.router, Method::GET, &format!("/api/backups/{backup_id}/download"), &new_token, None, Vec::new(),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded_again, downloaded);

    let (status, deleted) = request_json(
        &test_app.router, Method::DELETE, &format!("/api/backups/{backup_id}"), Some(&new_token), None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["data"]["deleted"], true);
    assert!(!std::path::Path::new(&path).exists());
    let (status, history) = request_json(&test_app.router, Method::GET, "/api/backups", Some(&new_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!history["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == backup_id));

    test_app.cleanup();
}

#[tokio::test]
async fn sqlite_records_and_theme_preference_persist_after_reopen() {
    let test_app = TestApp::new();
    let data_dir = test_app.data_dir.clone();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let persisted_arabic_name = "انور النعاس";
    create_worker(&test_app.router, &manager_token, persisted_arabic_name).await;

    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        "/api/preferences/theme",
        Some(&manager_token),
        Some(json!({ "theme": "dark" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    drop(test_app);
    let reopened = build_router(create_state(data_dir.clone()).expect("database should reopen"));
    let new_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (status, user) = request_json(
        &reopened,
        Method::GET,
        "/api/auth/me",
        Some(&new_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["data"]["theme"], "dark");

    let (status, workers) = request_json(
        &reopened,
        Method::GET,
        "/api/workers",
        Some(&new_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let persisted_workers = workers["data"]["items"]
        .as_array()
        .expect("worker list");
    assert_eq!(persisted_workers.len(), 1);
    assert_eq!(persisted_workers[0]["fullName"], persisted_arabic_name);

    drop(reopened);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn financial_flow_handles_configurable_commission_showroom_and_expense_allocations() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_one = create_worker(&test_app.router, &manager_token, "العامل الأول").await;
    let worker_two = create_worker(&test_app.router, &manager_token, "العامل الثاني").await;

    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        "/api/settings",
        Some(&manager_token),
        Some(json!({ "defaultWorkerCommissionBps": 4000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, showroom) = request_json(
        &test_app.router,
        Method::POST,
        "/api/showrooms",
        Some(&manager_token),
        Some(json!({ "name": "معرض الاختبار", "isActive": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"]
        .as_str()
        .expect("showroom ID")
        .to_owned();

    create_cash_wash(&test_app.router, &manager_token, &worker_one, "50").await;
    let (status, showroom_wash) = request_json(
        &test_app.router,
        Method::POST,
        "/api/washes",
        Some(&manager_token),
        Some(json!({
            "vehicleMake": "BMW", "vehicleModel": "X5", "price": "100", "workerId": worker_two,
            "paymentType": "showroom", "showroomId": showroom_id, "showroomPaymentMethod": "cash", "occurredAt": "2026-08-29T12:00:00Z", "clientRequestId": Uuid::new_v4().to_string()
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(showroom_wash["data"]["duplicate"], false);

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/showroom-payments",
        Some(&manager_token),
        Some(
            json!({ "showroomId": showroom_id, "amount": "30", "paidAt": "2026-08-29T14:00:00Z" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for expense in [
        json!({ "description": "إيجار", "category": "إيجار", "amount": "10", "occurredAt": "2026-08-29T15:00:00Z", "allocationType": "business" }),
        json!({ "description": "مواد عمال", "category": "مواد", "amount": "4", "occurredAt": "2026-08-29T15:10:00Z", "allocationType": "workers" }),
        json!({ "description": "منظفات مشتركة", "category": "مواد", "amount": "10", "occurredAt": "2026-08-29T15:20:00Z", "allocationType": "shared", "businessBps": 5000 }),
    ] {
        let (status, _) = request_json(
            &test_app.router,
            Method::POST,
            "/api/expenses",
            Some(&manager_token),
            Some(expense),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, finance) = request_json(
        &test_app.router,
        Method::GET,
        &all_time_endpoint("/api/finance/overview"),
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary = &finance["data"];
    assert_eq!(summary["totalWashRevenueMilli"], 150_000);
    assert_eq!(
        summary["workerCommissionsMilli"], 60_000,
        "40% of each wash"
    );
    assert_eq!(summary["businessShareMilli"], 90_000);
    assert_eq!(
        summary["businessExpensesMilli"], 15_000,
        "business-only plus its 50% shared portion"
    );
    assert_eq!(
        summary["workerExpensesMilli"], 9_000,
        "workers-only plus workers' 50% shared portion"
    );
    assert_eq!(
        summary["netBusinessProfitMilli"], 75_000,
        "workers' expense portion is never deducted from business profit"
    );
    assert_eq!(summary["outstandingShowroomDebtMilli"], 70_000);

    test_app.cleanup();
}

#[tokio::test]
async fn manager_created_wash_uses_default_fifty_percent_commission() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "محمد العامل").await;
    create_cash_wash(&test_app.router, &manager_token, &worker_id, "50").await;

    let (status, payload) = request_json(
        &test_app.router,
        Method::GET,
        "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z",
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let wash = &payload["data"]["items"][0];
    assert_eq!(wash["priceMilli"], 50_000);
    assert_eq!(wash["commissionBps"], 5_000);
    assert_eq!(wash["commissionMilli"], 25_000);
    assert_eq!(wash["businessShareMilli"], 25_000);

    let (status, payload) = request_json(
        &test_app.router,
        Method::GET,
        &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")),
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["grossCommissionMilli"], 25_000);
    assert_eq!(payload["data"]["remainingMilli"], 25_000);

    test_app.cleanup();
}

#[tokio::test]
async fn employee_account_is_independent_from_workers_and_financial_access() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_a = create_worker(&test_app.router, &manager_token, "أحمد العامل").await;
    let worker_b = create_worker(&test_app.router, &manager_token, "سالم العامل").await;
    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/workers/{worker_a}"),
        Some(&manager_token),
        Some(json!({
            "fullName": "أحمد العامل",
            "commissionBpsOverride": 3750,
            "isActive": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    create_cash_wash(&test_app.router, &manager_token, &worker_a, "80").await;

    let (status, payload) = request_json(
        &test_app.router,
        Method::POST,
        "/api/users",
        Some(&manager_token),
        Some(json!({
            "fullName": "موظف الاختبار",
            "username": "employee.test",
            "password": EMPLOYEE_PASSWORD,
            "roleCode": "employee",
            "workerId": worker_a,
            "isActive": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(payload["data"]["id"].is_string());
    let employee_id = payload["data"]["id"].as_str().unwrap();

    let employee_token = login(&test_app.router, "employee.test", EMPLOYEE_PASSWORD).await;

    let (status, me) = request_json(
        &test_app.router,
        Method::GET,
        "/api/auth/me",
        Some(&employee_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(me["data"].get("workerId").is_none());

    let (status, users) = request_json(
        &test_app.router,
        Method::GET,
        "/api/users",
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let employee = users["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["id"] == employee_id)
        .unwrap();
    assert!(employee.get("workerId").is_none());

    let financial_paths = vec![
        "/api/finance/overview".to_owned(),
        "/api/reports/financial".to_owned(),
        "/api/expenses".to_owned(),
        "/api/showroom-payments".to_owned(),
        "/api/settings".to_owned(),
        "/api/audit-logs".to_owned(),
        "/api/backups".to_owned(),
        "/api/showrooms/no-such-showroom/financial".to_owned(),
        format!("/api/workers/{worker_a}/financial?date=2026-08-29"),
    ];
    for path in financial_paths {
        let (status, _) = request_json(
            &test_app.router,
            Method::GET,
            &path,
            Some(&employee_token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "employee accessed {path}");
    }

    for path in [
        "/api/dashboard".to_owned(),
        "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z".to_owned(),
        "/api/reports/operational".to_owned(),
    ] {
        let (status, payload) = request_json(
            &test_app.router,
            Method::GET,
            &path,
            Some(&employee_token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "employee should access {path}");
        assert_no_sensitive_financial_keys(&payload["data"]);
    }

    let (status, workers) = request_json(&test_app.router, Method::GET, "/api/workers?date=2026-08-29", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workers["data"]["items"].as_array().unwrap().len(), 2);
    for worker in workers["data"]["items"].as_array().unwrap() {
        assert_no_sensitive_financial_keys(worker);
    }

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/workers",
        Some(&employee_token),
        Some(json!({
            "fullName": "عامل بعمولة غير مخولة",
            "commissionBpsOverride": 5000,
            "isActive": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/workers/{worker_a}"),
        Some(&employee_token),
        Some(json!({
            "fullName": "أحمد العامل",
            "commissionBpsOverride": 5000,
            "isActive": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/workers/{worker_a}"),
        Some(&employee_token),
        Some(json!({
            "fullName": "أحمد العامل",
            "commissionBpsOverride": null,
            "isActive": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, worker_a_financial) = request_json(
        &test_app.router,
        Method::GET,
        &format!("/api/workers/{worker_a}/financial?date=2026-08-29"),
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(worker_a_financial["data"]["commissionBpsOverride"], 3750);

    let (status, worker_a_detail) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}?date=2026-08-29"), Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(worker_a_detail["data"]["history"].as_array().unwrap().len(), 1);
    assert_eq!(worker_a_detail["data"]["history"][0]["priceMilli"], 80_000);
    assert!(worker_a_detail["data"].get("dailyValue").is_some());

    let (status, worker_b_detail) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_b}?date=2026-08-29"), Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(worker_b_detail["data"]["history"].as_array().unwrap().is_empty());

    test_app.cleanup();
}

#[tokio::test]
async fn role_permission_switches_change_real_api_access_immediately() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/users",
        Some(&manager_token),
        Some(json!({
            "fullName": "موظف الصلاحيات",
            "username": "permissions.employee",
            "password": EMPLOYEE_PASSWORD,
            "roleCode": "employee",
            "isActive": true,
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_token = login(&test_app.router, "permissions.employee", EMPLOYEE_PASSWORD).await;

    let (status, roles) = request_json(&test_app.router, Method::GET, "/api/roles", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let employee_role_id = roles["data"]["items"].as_array().unwrap().iter()
        .find(|role| role["code"] == "employee").unwrap()["id"].as_str().unwrap().to_owned();

    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        &format!("/api/roles/{employee_role_id}/permissions"),
        Some(&manager_token),
        Some(json!({ "permissionCodes": ["financial.manage"] })),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (status, me) = request_json(&test_app.router, Method::GET, "/api/auth/me", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["data"]["permissions"], json!(["financial.manage"]));
    let (status, _) = request_json(&test_app.router, Method::GET, "/api/finance/overview", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        &format!("/api/roles/{employee_role_id}/permissions"),
        Some(&manager_token),
        Some(json!({ "permissionCodes": [] })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::GET, "/api/finance/overview", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    test_app.cleanup();
}

#[tokio::test]
async fn individual_employee_permissions_override_role_defaults() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let mut employees = Vec::new();
    for (name, username) in [("الموظف الأول", "first.employee"), ("الموظف الثاني", "second.employee")] {
        let (status, payload) = request_json(
            &test_app.router, Method::POST, "/api/users", Some(&manager_token),
            Some(json!({ "fullName": name, "username": username, "password": EMPLOYEE_PASSWORD, "roleCode": "employee", "isActive": true })),
        ).await;
        assert_eq!(status, StatusCode::OK);
        employees.push((payload["data"]["id"].as_str().unwrap().to_owned(), login(&test_app.router, username, EMPLOYEE_PASSWORD).await));
    }

    let (status, _) = request_json(
        &test_app.router, Method::PUT, &format!("/api/users/{}/permissions", employees[0].0), Some(&manager_token),
        Some(json!({ "permissionCodes": ["financial.manage"] })),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (first_status, _) = request_json(&test_app.router, Method::GET, "/api/finance/overview", Some(&employees[0].1), None).await;
    let (second_status, _) = request_json(&test_app.router, Method::GET, "/api/finance/overview", Some(&employees[1].1), None).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::FORBIDDEN);
    let (first_dashboard, _) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employees[0].1), None).await;
    let (second_dashboard, _) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employees[1].1), None).await;
    assert_eq!(first_dashboard, StatusCode::FORBIDDEN);
    assert_eq!(second_dashboard, StatusCode::OK);

    test_app.cleanup();
}

#[tokio::test]
async fn daily_revenue_permission_controls_dashboard_card_data_per_employee() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل الإيراد اليومي").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض الإيراد اليومي","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap().to_owned();
    let (status, created) = request_json(
        &test_app.router, Method::POST, "/api/users", Some(&manager_token),
        Some(json!({"fullName":"موظف الإيراد اليومي","username":"daily.revenue.employee","password":EMPLOYEE_PASSWORD,"roleCode":"employee","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_id = created["data"]["id"].as_str().unwrap().to_owned();
    let employee_token = login(&test_app.router, "daily.revenue.employee", EMPLOYEE_PASSWORD).await;

    let (status, dashboard_off) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dashboard_off["data"].get("financial").is_none());

    let (status, _) = request_json(
        &test_app.router, Method::PUT, &format!("/api/users/{employee_id}/permissions"), Some(&manager_token),
        Some(json!({"permissionCodes":["operational.read","operational.write","dashboard.daily_revenue.read"]})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let today = business_today_key();
    create_cash_wash_at(&test_app.router, &employee_token, &worker_id, "80", &format!("{today}T10:00:00Z")).await;
    let (status, _) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&employee_token),
        Some(json!({"vehicleMake":"BMW","vehicleModel":"X5","price":"100","workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"cash","occurredAt":format!("{today}T12:00:00Z"),"clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (status, dashboard_on) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dashboard_on["data"]["financial"]["todayRevenue"], 180_000);
    assert!(dashboard_on["data"]["financial"].get("todayCustomerRevenue").is_none());
    assert!(dashboard_on["data"]["financial"].get("todayNetProfit").is_none());
    assert!(dashboard_on["data"]["financial"].get("todayShowroomRevenue").is_none());
    assert!(dashboard_on["data"]["financial"].get("todayShowroomNetProfit").is_none());

    let (status, _) = request_json(
        &test_app.router, Method::PUT, &format!("/api/users/{employee_id}/permissions"), Some(&manager_token),
        Some(json!({"permissionCodes":["operational.read","operational.write"]})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, dashboard_disabled_again) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_token), None).await;
    assert!(dashboard_disabled_again["data"]["financial"].get("todayRevenue").is_none());
    assert!(dashboard_disabled_again["data"]["financial"].get("todayShowroomRevenue").is_none());

    let (_, manager_dashboard) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&manager_token), None).await;
    assert_eq!(manager_dashboard["data"]["financial"]["todayRevenue"], 180_000);
    assert_eq!(manager_dashboard["data"]["financial"]["todayCustomerRevenue"], 80_000);
    assert_eq!(manager_dashboard["data"]["financial"]["todayNetProfit"], 40_000);
    assert_eq!(manager_dashboard["data"]["financial"]["todayShowroomRevenue"], 100_000);
    assert_eq!(manager_dashboard["data"]["financial"]["todayShowroomNetProfit"], 50_000);
    test_app.cleanup();
}

#[tokio::test]
async fn worker_daily_value_is_permission_scoped_and_stored_per_worker_and_date() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_a = create_worker(&test_app.router, &manager_token, "عامل القيمة أ").await;
    let worker_b = create_worker(&test_app.router, &manager_token, "عامل القيمة ب").await;
    let business_now = Utc::now() + Duration::hours(2);
    let today = business_now.format("%Y-%m-%d").to_string();
    let tomorrow = (business_now + Duration::days(1)).format("%Y-%m-%d").to_string();

    let (status, _) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, manager_before) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}"), Some(&manager_token), None).await;
    assert_eq!(manager_before["data"]["dailyValue"]["amountMilli"], Value::Null);

    let (status, created) = request_json(
        &test_app.router, Method::POST, "/api/users", Some(&manager_token),
        Some(json!({"fullName":"موظف القيمة اليومية","username":"daily.value.employee","password":EMPLOYEE_PASSWORD,"roleCode":"employee","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_id = created["data"]["id"].as_str().unwrap();
    let employee_token = login(&test_app.router, "daily.value.employee", EMPLOYEE_PASSWORD).await;
    let (status, _) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}"), Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::PUT, &format!("/api/workers/{worker_a}/daily-value"), Some(&employee_token), Some(json!({"valueDate":today,"amount":"100"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request_json(&test_app.router, Method::PUT, &format!("/api/users/{employee_id}/permissions"), Some(&manager_token), Some(json!({"permissionCodes":["operational.read","worker.daily_value.manage"]}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::PUT, &format!("/api/workers/{worker_a}/daily-value"), Some(&employee_token), Some(json!({"valueDate":today,"amount":"100"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, employee_after) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}"), Some(&employee_token), None).await;
    assert_eq!(employee_after["data"]["dailyValue"]["amountMilli"], 100_000);
    let (status, _) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_b}"), Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::PUT, &format!("/api/workers/{worker_b}/daily-value"), Some(&employee_token), Some(json!({"valueDate":today,"amount":"150"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(&test_app.router, Method::PUT, &format!("/api/workers/{worker_a}/daily-value"), Some(&employee_token), Some(json!({"valueDate":tomorrow,"amount":"200"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, manager_after) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_a}"), Some(&manager_token), None).await;
    assert_eq!(manager_after["data"]["dailyValue"]["amountMilli"], 100_000);
    test_app.cleanup();
}

#[tokio::test]
async fn dashboard_revenue_uses_local_today_and_recalculates_after_wash_mutations() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل مزامنة الإيراد").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض مزامنة الإيراد","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap();

    // Omitting occurredAt uses the backend's current UTC timestamp. The
    // dashboard must classify it by the server's local calendar day.
    let (status, cash) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Camry","price":"50","workerId":worker_id,"paymentType":"cash","clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let cash_id = cash["data"]["id"].as_str().unwrap();
    let (status, showroom_wash) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({"vehicleMake":"BMW","vehicleModel":"X5","price":"50","workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"cash","clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id_wash = showroom_wash["data"]["id"].as_str().unwrap();

    let (_, initial) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&manager_token), None).await;
    assert_eq!(initial["data"]["financial"]["todayRevenue"], 100_000);
    assert_eq!(initial["data"]["financial"]["todayCustomerRevenue"], 50_000);
    assert_eq!(initial["data"]["financial"]["todayShowroomRevenue"], 50_000);
    assert_eq!(initial["data"]["financial"]["todayNetProfit"], 25_000);
    assert_eq!(initial["data"]["financial"]["todayShowroomNetProfit"], 25_000);

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{cash_id}"), Some(&manager_token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Camry","price":"70","workerId":worker_id,"paymentType":"cash"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{showroom_id_wash}"), Some(&manager_token),
        Some(json!({"vehicleMake":"BMW","vehicleModel":"X5","price":"80","workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"cash"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, edited) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&manager_token), None).await;
    assert_eq!(edited["data"]["financial"]["todayRevenue"], 150_000);
    assert_eq!(edited["data"]["financial"]["todayCustomerRevenue"], 70_000);
    assert_eq!(edited["data"]["financial"]["todayShowroomRevenue"], 80_000);

    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{cash_id}/void"), Some(&manager_token), Some(json!({"reason":"اختبار المزامنة"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_cash_void) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&manager_token), None).await;
    assert_eq!(after_cash_void["data"]["financial"]["todayRevenue"], 80_000);
    assert_eq!(after_cash_void["data"]["financial"]["todayCustomerRevenue"], 0);
    assert_eq!(after_cash_void["data"]["financial"]["todayShowroomRevenue"], 80_000);

    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{showroom_id_wash}/void"), Some(&manager_token), Some(json!({"reason":"اختبار المزامنة"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_all_void) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&manager_token), None).await;
    assert_eq!(after_all_void["data"]["financial"]["todayRevenue"], 0);
    assert_eq!(after_all_void["data"]["financial"]["todayCustomerRevenue"], 0);
    assert_eq!(after_all_void["data"]["financial"]["todayShowroomRevenue"], 0);
    test_app.cleanup();
}

#[tokio::test]
async fn dashboard_selected_date_uses_tripoli_boundaries_and_account_scope() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل تاريخ لوحة المتابعة").await;
    let (status, employee) = request_json(
        &test_app.router,
        Method::POST,
        "/api/users",
        Some(&manager_token),
        Some(json!({
            "fullName":"موظف تاريخ لوحة المتابعة",
            "username":"dashboard.date.employee",
            "password":EMPLOYEE_PASSWORD,
            "roleCode":"employee",
            "isActive":true
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_id = employee["data"]["id"].as_str().unwrap();
    let (status, _) = request_json(
        &test_app.router,
        Method::PUT,
        &format!("/api/users/{employee_id}/permissions"),
        Some(&manager_token),
        Some(json!({"permissionCodes":["operational.read","operational.write","dashboard.daily_revenue.read"]})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_token = login(&test_app.router, "dashboard.date.employee", EMPLOYEE_PASSWORD).await;

    let business_today = (Utc::now() + Duration::hours(2)).date_naive();
    let selected = business_today - Duration::days(1);
    let selected_key = selected.format("%Y-%m-%d").to_string();
    let next_key = business_today.format("%Y-%m-%d").to_string();
    let selected_start_utc = selected.and_hms_opt(0, 0, 0).unwrap() - Duration::hours(2);
    let selected_morning = format!("{}Z", (selected_start_utc + Duration::hours(10)).format("%Y-%m-%dT%H:%M:%S"));
    let selected_evening = format!("{}Z", (selected_start_utc + Duration::hours(23) + Duration::minutes(59)).format("%Y-%m-%dT%H:%M:%S"));
    let next_midnight = format!("{}Z", (selected_start_utc + Duration::hours(24)).format("%Y-%m-%dT%H:%M:%S"));

    create_cash_wash_at(&test_app.router, &employee_token, &worker_id, "40", &selected_morning).await;
    create_cash_wash_at(&test_app.router, &employee_token, &worker_id, "60", &selected_evening).await;
    create_cash_wash_at(&test_app.router, &employee_token, &worker_id, "90", &next_midnight).await;
    create_cash_wash_at(&test_app.router, &manager_token, &worker_id, "30", &selected_morning).await;

    let selected_endpoint = format!("/api/dashboard?date={selected_key}");
    let (status, employee_selected) = request_json(&test_app.router, Method::GET, &selected_endpoint, Some(&employee_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(employee_selected["data"]["selectedDate"], selected_key);
    assert_eq!(employee_selected["data"]["businessTimeZone"], "Africa/Tripoli");
    assert_eq!(employee_selected["data"]["todayWashes"], 2);
    assert_eq!(employee_selected["data"]["recentWashes"].as_array().unwrap().len(), 2);
    assert_eq!(employee_selected["data"]["financial"]["todayRevenue"], 100_000);

    let (_, manager_selected) = request_json(&test_app.router, Method::GET, &selected_endpoint, Some(&manager_token), None).await;
    assert_eq!(manager_selected["data"]["todayWashes"], 3);
    assert_eq!(manager_selected["data"]["recentWashes"].as_array().unwrap().len(), 3);
    assert_eq!(manager_selected["data"]["financial"]["todayRevenue"], 130_000);

    let next_endpoint = format!("/api/dashboard?date={next_key}");
    let (_, employee_next) = request_json(&test_app.router, Method::GET, &next_endpoint, Some(&employee_token), None).await;
    assert_eq!(employee_next["data"]["todayWashes"], 1);
    assert_eq!(employee_next["data"]["financial"]["todayRevenue"], 90_000);

    let future = (business_today + Duration::days(1)).format("%Y-%m-%d");
    let (status, _) = request_json(&test_app.router, Method::GET, &format!("/api/dashboard?date={future}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    test_app.cleanup();
}

#[tokio::test]
async fn global_working_date_filters_all_daily_endpoints_without_mutating_records() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل التاريخ العالمي").await;
    let business_today = (Utc::now() + Duration::hours(2)).date_naive();
    let selected = business_today - Duration::days(2);
    let following = selected + Duration::days(1);
    let selected_key = selected.format("%Y-%m-%d").to_string();
    let following_key = following.format("%Y-%m-%d").to_string();
    let selected_start_utc = selected.and_hms_opt(0, 0, 0).unwrap() - Duration::hours(2);
    let selected_morning = format!("{}Z", (selected_start_utc + Duration::hours(8)).format("%Y-%m-%dT%H:%M:%S"));
    let selected_evening = format!("{}Z", (selected_start_utc + Duration::hours(23) + Duration::minutes(59)).format("%Y-%m-%dT%H:%M:%S"));
    let following_midnight = format!("{}Z", (selected_start_utc + Duration::hours(24)).format("%Y-%m-%dT%H:%M:%S"));
    let payroll_month = &selected_key[..7];
    let (status, payroll_employee) = request_json(
        &test_app.router, Method::POST, "/api/payroll/employees", Some(&manager_token),
        Some(json!({"fullName":"موظف التاريخ العالمي","month":payroll_month,"salary":"100"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let payroll_employee_id = payroll_employee["data"]["employee"]["id"].as_str().unwrap().to_owned();

    let (status, first) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Global A","price":"30","workerId":worker_id,"paymentType":"cash","occurredAt":selected_morning,"clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let first_id = first["data"]["id"].as_str().unwrap().to_owned();
    let (status, second) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({"vehicleMake":"Toyota","vehicleModel":"Global B","price":"40","workerId":worker_id,"paymentType":"cash","occurredAt":selected_evening,"clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let second_id = second["data"]["id"].as_str().unwrap().to_owned();
    create_cash_wash_at(&test_app.router, &manager_token, &worker_id, "90", &following_midnight).await;

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{second_id}/overnight"), Some(&manager_token),
        Some(json!({"isOvernight":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{second_id}/paid"), Some(&manager_token),
        Some(json!({"isPaid":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);

    for (date, amount) in [(&selected_key, "110"), (&following_key, "220")] {
        let (status, _) = request_json(
            &test_app.router, Method::PUT, &format!("/api/workers/{worker_id}/daily-value"), Some(&manager_token),
            Some(json!({"valueDate":date,"amount":amount})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    for (occurred_at, amount) in [(&selected_morning, "5"), (&following_midnight, "9")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/expenses", Some(&manager_token),
            Some(json!({"description":"مصروف التاريخ العالمي","category":"اختبار","amount":amount,"occurredAt":occurred_at,"allocationType":"business"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    for (occurred_at, amount) in [(&selected_morning, "500"), (&following_midnight, "900")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
            Some(json!({"transactionType":"withdrawal","amount":amount,"occurredAt":occurred_at,"notes":"حركة التاريخ العالمي"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    for (withdrawn_at, amount) in [(&selected_morning, "25"), (&following_midnight, "35")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/payroll/withdrawals", Some(&manager_token),
            Some(json!({"employeeId":payroll_employee_id,"amount":amount,"withdrawnAt":withdrawn_at,"notes":"مسحوب التاريخ العالمي"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    for (deducted_at, amount) in [(&selected_evening, "7"), (&following_midnight, "11")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/payroll/deductions", Some(&manager_token),
            Some(json!({"employeeId":payroll_employee_id,"amount":amount,"deductedAt":deducted_at,"notes":"خصم التاريخ العالمي"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض التاريخ العالمي","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap().to_owned();
    for (occurred_at, amount) in [(&selected_morning, "55"), (&following_midnight, "65")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
            Some(json!({
                "vehicleMake":"Toyota","vehicleModel":"Dealer Global","price":amount,"workerId":worker_id,
                "paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"bank",
                "occurredAt":occurred_at,"clientRequestId":Uuid::new_v4().to_string()
            })),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    for (paid_at, amount) in [(&selected_evening, "15"), (&following_midnight, "20")] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/showroom-payments", Some(&manager_token),
            Some(json!({"showroomId":showroom_id,"amount":amount,"paidAt":paid_at,"notes":"دفعة التاريخ العالمي"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, backup) = request_json(
        &test_app.router, Method::POST, "/api/backups", Some(&manager_token), Some(json!({})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let backup_id = backup["data"]["id"].as_str().unwrap().to_owned();
    let historical_audit_id = Uuid::new_v4().to_string();
    {
        let connection = Connection::open(test_app.data_dir.join("carwash.db")).unwrap();
        connection.execute(
            "UPDATE backup_history SET created_at=?1 WHERE id=?2",
            params![selected_evening, backup_id],
        ).unwrap();
        connection.execute(
            "INSERT INTO audit_logs(id,user_id,action,entity_type,entity_id,description,metadata_json,created_at) VALUES(?1,NULL,'GLOBAL_DATE_TEST','test',NULL,'historical audit fixture',NULL,?2)",
            params![historical_audit_id, selected_evening],
        ).unwrap();
    }

    let selected_query = format!("date={selected_key}");
    let (_, dashboard) = request_json(&test_app.router, Method::GET, &format!("/api/dashboard?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(dashboard["data"]["todayWashes"], 3);
    assert_eq!(dashboard["data"]["financial"]["todayRevenue"], 125_000);
    let (_, washes) = request_json(&test_app.router, Method::GET, &format!("/api/washes?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(washes["data"]["items"].as_array().unwrap().len(), 2);
    assert!(washes["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == first_id));
    let (_, paid) = request_json(&test_app.router, Method::GET, &format!("/api/paid-cars?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(paid["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(paid["data"]["settlementMilli"], 40_000);
    let (_, overnight) = request_json(&test_app.router, Method::GET, &format!("/api/overnight-cars?{selected_query}"), Some(&manager_token), None).await;
    assert!(overnight["data"]["items"].as_array().unwrap().is_empty(), "completed overnight cars are not active");
    let (_, workers) = request_json(&test_app.router, Method::GET, &format!("/api/workers?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(workers["data"]["items"][0]["washCount"], 3);
    let (_, worker) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(worker["data"]["history"].as_array().unwrap().len(), 3);
    assert_eq!(worker["data"]["dailyValue"]["amountMilli"], 110_000);
    let (_, worker_financial) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}/financial?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(worker_financial["data"]["grossCommissionMilli"], 62_500);
    let (_, finance) = request_json(&test_app.router, Method::GET, &format!("/api/finance/overview?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(finance["data"]["totalWashRevenueMilli"], 125_000);
    assert_eq!(finance["data"]["expensesMilli"], 5_000);
    let (_, expenses) = request_json(&test_app.router, Method::GET, &format!("/api/expenses?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(expenses["data"]["items"].as_array().unwrap().len(), 1);
    let (_, worker_movements) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}/withdrawals-returns?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(worker_movements["data"]["totalWithdrawalsMilli"], 500_000);
    assert_eq!(worker_movements["data"]["transactions"].as_array().unwrap().len(), 1);
    let selected_month = selected.format("%Y-%m");
    let (_, salary_withdrawals) = request_json(&test_app.router, Method::GET, &format!("/api/payroll/withdrawals?month={selected_month}&{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(salary_withdrawals["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(salary_withdrawals["data"]["items"][0]["amountMilli"], 25_000);
    let (_, salary_deductions) = request_json(&test_app.router, Method::GET, &format!("/api/payroll/deductions?month={selected_month}&{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(salary_deductions["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(salary_deductions["data"]["items"][0]["amountMilli"], 7_000);
    let (_, showrooms) = request_json(&test_app.router, Method::GET, &format!("/api/showrooms?{selected_query}"), Some(&manager_token), None).await;
    let selected_showroom = showrooms["data"]["items"].as_array().unwrap().iter().find(|item| item["id"] == showroom_id).unwrap();
    assert_eq!(selected_showroom["washCount"], 1);
    assert_eq!(selected_showroom["financial"]["paymentsMilli"], 15_000);
    let (_, showroom_detail) = request_json(&test_app.router, Method::GET, &format!("/api/showrooms/{showroom_id}?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(showroom_detail["data"]["history"].as_array().unwrap().len(), 1);
    let (_, showroom_financial) = request_json(&test_app.router, Method::GET, &format!("/api/showrooms/{showroom_id}/financial?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(showroom_financial["data"]["chargesMilli"], 55_000);
    assert_eq!(showroom_financial["data"]["paymentsMilli"], 15_000);
    let (_, showroom_payments) = request_json(&test_app.router, Method::GET, &format!("/api/showroom-payments?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(showroom_payments["data"]["items"].as_array().unwrap().len(), 1);
    let (_, showroom_debt) = request_json(&test_app.router, Method::GET, &format!("/api/showroom-debts/{showroom_id}?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(showroom_debt["data"]["operations"].as_array().unwrap().len(), 1);
    assert_eq!(showroom_debt["data"]["payments"].as_array().unwrap().len(), 1);
    let (_, audit) = request_json(&test_app.router, Method::GET, &format!("/api/audit-logs?{selected_query}&limit=500"), Some(&manager_token), None).await;
    assert!(audit["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == historical_audit_id));
    let (_, backups) = request_json(&test_app.router, Method::GET, &format!("/api/backups?{selected_query}"), Some(&manager_token), None).await;
    assert!(backups["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == backup_id));

    // The paid-car action is a reversible status update, never a destructive delete.
    let (status, reverted) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{second_id}/paid?{selected_query}"),
        Some(&manager_token),
        Some(json!({"isPaid":false})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reverted["data"]["wash"]["isPaid"], false);
    assert_eq!(reverted["data"]["settlementMilli"], 0);
    let (_, paid_after_revert) = request_json(&test_app.router, Method::GET, &format!("/api/paid-cars?{selected_query}"), Some(&manager_token), None).await;
    assert!(paid_after_revert["data"]["items"].as_array().unwrap().is_empty());
    assert_eq!(paid_after_revert["data"]["settlementMilli"], 0);
    let (_, latest_after_revert) = request_json(&test_app.router, Method::GET, &format!("/api/washes?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(latest_after_revert["data"]["items"].as_array().unwrap().len(), 2);
    assert!(!latest_after_revert["data"]["items"].as_array().unwrap().iter().any(|item| item["id"] == second_id));
    let (_, overnight_after_revert) = request_json(&test_app.router, Method::GET, &format!("/api/overnight-cars?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(overnight_after_revert["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(overnight_after_revert["data"]["items"][0]["wash"]["id"], second_id);

    let (_, following_dashboard) = request_json(&test_app.router, Method::GET, &format!("/api/dashboard?date={following_key}"), Some(&manager_token), None).await;
    assert_eq!(following_dashboard["data"]["todayWashes"], 2);
    assert_eq!(following_dashboard["data"]["financial"]["todayRevenue"], 155_000);
    let (_, selected_again) = request_json(&test_app.router, Method::GET, &format!("/api/dashboard?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(selected_again["data"]["todayWashes"], 3, "changing the viewing date must not move or edit records");
    let (_, default_today_washes) = request_json(&test_app.router, Method::GET, "/api/washes", Some(&manager_token), None).await;
    assert!(default_today_washes["data"]["items"].as_array().unwrap().is_empty(), "omitting date must fall back to the actual business day, never an unbounded history");

    let future = (business_today + Duration::days(1)).format("%Y-%m-%d");
    let (status, _) = request_json(&test_app.router, Method::GET, &format!("/api/washes?date={future}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let connection = Connection::open(test_app.data_dir.join("carwash.db")).unwrap();
    let unchanged_operation_time: String = connection.query_row(
        "SELECT occurred_at FROM wash_operations WHERE id=?1", [&first_id], |row| row.get(0),
    ).unwrap();
    let unchanged_paid_time: Option<String> = connection.query_row(
        "SELECT paid_at FROM wash_operations WHERE id=?1", [&second_id], |row| row.get(0),
    ).unwrap();
    let unchanged_overnight_time: String = connection.query_row(
        "SELECT marked_at FROM overnight_cars WHERE wash_id=?1", [&second_id], |row| row.get(0),
    ).unwrap();
    assert_eq!(unchanged_operation_time, selected_morning);
    assert!(unchanged_paid_time.is_none(), "reverting paid status clears only paid metadata");
    assert_ne!(unchanged_overnight_time, selected_evening, "marking metadata may reflect the action time without changing the operation date");
    drop(connection);
    test_app.cleanup();
}

#[tokio::test]
async fn only_managers_can_safely_delete_workers_without_breaking_history() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل الحذف الآمن").await;
    create_cash_wash(&test_app.router, &manager_token, &worker_id, "80").await;
    let (status, created) = request_json(
        &test_app.router, Method::POST, "/api/users", Some(&manager_token),
        Some(json!({"fullName":"موظف لا يحذف","username":"worker.delete.employee","password":EMPLOYEE_PASSWORD,"roleCode":"employee","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_token = login(&test_app.router, "worker.delete.employee", EMPLOYEE_PASSWORD).await;
    let (status, _) = request_json(
        &test_app.router, Method::DELETE, &format!("/api/workers/{worker_id}"), Some(&employee_token), None,
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, deleted) = request_json(
        &test_app.router, Method::DELETE, &format!("/api/workers/{worker_id}"), Some(&manager_token), None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["data"]["archived"], true);

    let (status, worker) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}")), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(worker["data"]["worker"]["isActive"], false);
    assert_eq!(worker["data"]["history"].as_array().unwrap().len(), 1);
    let (status, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(financial["data"]["paidMilli"], 0);
    assert_eq!(financial["data"]["remainingMilli"], 40_000);
    assert!(created["data"]["id"].is_string());
    test_app.cleanup();
}

#[tokio::test]
async fn worker_deletion_and_wash_cancellation_persist_in_active_lists() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل يختفي نهائيًا").await;
    let wash_id = create_cash_wash(&test_app.router, &manager_token, &worker_id, "60").await;

    let (_, workers_before) = request_json(&test_app.router, Method::GET, "/api/workers", Some(&manager_token), None).await;
    assert!(workers_before["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == worker_id));
    let (_, washes_before) = request_json(&test_app.router, Method::GET, "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z", Some(&manager_token), None).await;
    assert!(washes_before["data"]["items"].as_array().unwrap().iter().any(|wash| wash["id"] == wash_id));

    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{wash_id}/void"), Some(&manager_token), Some(json!({"reason":"إلغاء الاختبار"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (second_status, second_payload) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{wash_id}/void"), Some(&manager_token), Some(json!({"reason":"طلب مكرر"}))).await;
    assert_eq!(second_status, StatusCode::BAD_REQUEST);
    assert_eq!(second_payload["error"], "هذه الغسلة ملغاة بالفعل");
    let (_, washes_after) = request_json(&test_app.router, Method::GET, "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z", Some(&manager_token), None).await;
    assert!(!washes_after["data"]["items"].as_array().unwrap().iter().any(|wash| wash["id"] == wash_id));

    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, workers_after) = request_json(&test_app.router, Method::GET, "/api/workers", Some(&manager_token), None).await;
    assert!(!workers_after["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == worker_id));
    let (_, workers_after_reload) = request_json(&test_app.router, Method::GET, "/api/workers?status=active", Some(&manager_token), None).await;
    assert!(!workers_after_reload["data"]["items"].as_array().unwrap().iter().any(|worker| worker["id"] == worker_id));

    let (_, historical_worker) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert_eq!(historical_worker["data"]["worker"]["isActive"], false);
    assert!(historical_worker["data"]["history"].as_array().unwrap().is_empty());

    test_app.cleanup();
}

#[tokio::test]
async fn employee_operations_are_account_scoped_and_user_deletion_preserves_history() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل الحسابات").await;
    let mut employee_ids = Vec::new();
    let mut employee_tokens = Vec::new();
    for (name, username) in [("الموظف أ", "account.employee.a"), ("الموظف ب", "account.employee.b")] {
        let (status, created) = request_json(
            &test_app.router,
            Method::POST,
            "/api/users",
            Some(&manager_token),
            Some(json!({"fullName":name,"username":username,"password":EMPLOYEE_PASSWORD,"roleCode":"employee","isActive":true})),
        ).await;
        assert_eq!(status, StatusCode::OK);
        employee_ids.push(created["data"]["id"].as_str().unwrap().to_owned());
        employee_tokens.push(login(&test_app.router, username, EMPLOYEE_PASSWORD).await);
    }

    let wash_a = create_employee_cash_wash(&test_app.router, &employee_tokens[0], &worker_id, "1111 أ ب").await;
    let wash_b = create_employee_cash_wash(&test_app.router, &employee_tokens[1], &worker_id, "2222 أ ب").await;

    let today = business_today_key();
    let endpoint = format!("/api/washes?from={today}T00:00:00Z&to={today}T23:59:59Z");
    let (_, employee_a_washes) = request_json(&test_app.router, Method::GET, &endpoint, Some(&employee_tokens[0]), None).await;
    let (_, employee_b_washes) = request_json(&test_app.router, Method::GET, &endpoint, Some(&employee_tokens[1]), None).await;
    assert_eq!(employee_a_washes["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(employee_b_washes["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(employee_a_washes["data"]["items"][0]["id"], wash_a);
    assert_eq!(employee_b_washes["data"]["items"][0]["id"], wash_b);
    assert_eq!(employee_a_washes["data"]["items"][0]["priceMilli"], 40_000);
    assert_eq!(employee_b_washes["data"]["items"][0]["priceMilli"], 40_000);

    let (_, employee_a_dashboard) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_tokens[0]), None).await;
    let (_, employee_b_dashboard) = request_json(&test_app.router, Method::GET, "/api/dashboard", Some(&employee_tokens[1]), None).await;
    assert_eq!(employee_a_dashboard["data"]["todayWashes"], 1);
    assert_eq!(employee_b_dashboard["data"]["todayWashes"], 1);
    let (_, manager_washes) = request_json(&test_app.router, Method::GET, &endpoint, Some(&manager_token), None).await;
    assert_eq!(manager_washes["data"]["items"].as_array().unwrap().len(), 2);
    let (_, worker_profile) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    let profile_history = worker_profile["data"]["history"].as_array().unwrap();
    assert_eq!(profile_history.len(), 2);
    assert!(profile_history.iter().all(|wash| wash["worker"]["id"] == worker_id && wash["worker"]["fullName"] == "عامل الحسابات"));
    let (_, refreshed_worker_profile) = request_json(&test_app.router, Method::GET, &format!("/api/workers/{worker_id}"), Some(&manager_token), None).await;
    assert!(refreshed_worker_profile["data"]["history"].as_array().unwrap().iter().all(|wash| wash["worker"]["id"] == worker_id && wash["worker"]["fullName"] == "عامل الحسابات"));

    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/users/{}", employee_ids[1]), Some(&employee_tokens[0]), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/users/{}", employee_ids[1]), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request_json(&test_app.router, Method::POST, "/api/auth/login", None, Some(json!({"username":"account.employee.b","password":EMPLOYEE_PASSWORD}))).await;
    assert_ne!(status, StatusCode::OK, "a deleted user must not be able to log in");
    let (status, users) = request_json(&test_app.router, Method::GET, "/api/users", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!users["data"]["items"].as_array().unwrap().iter().any(|user| user["id"] == employee_ids[1]));
    let (_, manager_washes_after_delete) = request_json(&test_app.router, Method::GET, &endpoint, Some(&manager_token), None).await;
    assert!(manager_washes_after_delete["data"]["items"].as_array().unwrap().iter().any(|wash| wash["id"] == wash_b));
    let data_dir = test_app.data_dir.clone();
    drop(test_app);
    let reopened = build_router(create_state(data_dir.clone()).expect("database should reopen after user deletion"));
    let restarted_manager_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, restarted_users) = request_json(&reopened, Method::GET, "/api/users", Some(&restarted_manager_token), None).await;
    assert!(!restarted_users["data"]["items"].as_array().unwrap().iter().any(|user| user["id"] == employee_ids[1]));
    let (deleted_login_status, _) = request_json(&reopened, Method::POST, "/api/auth/login", None, Some(json!({"username":"account.employee.b","password":EMPLOYEE_PASSWORD}))).await;
    assert_ne!(deleted_login_status, StatusCode::OK);
    let (_, restarted_washes) = request_json(&reopened, Method::GET, &endpoint, Some(&restarted_manager_token), None).await;
    assert!(restarted_washes["data"]["items"].as_array().unwrap().iter().any(|wash| wash["id"] == wash_b));
    drop(reopened);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn worker_withdrawal_returns_are_isolated_and_wash_deletion_recalculates_profile() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_a = create_worker(&test_app.router, &manager_token, "عامل السجل أ").await;
    let worker_b = create_worker(&test_app.router, &manager_token, "عامل السجل ب").await;
    let wash_a = create_cash_wash(&test_app.router, &manager_token, &worker_a, "50").await;
    let _wash_b = create_cash_wash(&test_app.router, &manager_token, &worker_b, "60").await;

    let (_, financial_before) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}/financial")), Some(&manager_token), None).await;
    assert_eq!(financial_before["data"]["grossCommissionMilli"], 25_000);

    for (worker_id, movement_type, amount, date) in [
        (&worker_a, "withdrawal", "500", "2026-08-10T12:00:00Z"),
        (&worker_a, "return", "100", "2026-08-11T12:00:00Z"),
        (&worker_b, "withdrawal", "50", "2026-08-12T12:00:00Z"),
    ] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
            Some(json!({"transactionType":movement_type,"amount":amount,"occurredAt":date,"notes":"حركة اختبار مستقلة"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (_, ledger_a) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}/withdrawals-returns")), Some(&manager_token), None).await;
    assert_eq!(ledger_a["data"]["totalWithdrawalsMilli"], 500_000);
    assert_eq!(ledger_a["data"]["totalReturnsMilli"], 100_000);
    assert_eq!(ledger_a["data"]["outstandingBalanceMilli"], 400_000);
    assert_eq!(ledger_a["data"]["transactions"].as_array().unwrap().len(), 2);
    let (_, ledger_b) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_b}/withdrawals-returns")), Some(&manager_token), None).await;
    assert_eq!(ledger_b["data"]["outstandingBalanceMilli"], 50_000);
    assert_eq!(ledger_b["data"]["transactions"].as_array().unwrap().len(), 1);

    let (_, financial_after_movements) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}/financial")), Some(&manager_token), None).await;
    assert_eq!(financial_after_movements["data"]["grossCommissionMilli"], 25_000, "withdrawals and returns must not affect earnings");

    let (_, profile_before) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}")), Some(&manager_token), None).await;
    assert_eq!(profile_before["data"]["history"].as_array().unwrap().len(), 1);
    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{wash_a}/void"), Some(&manager_token), Some(json!({"reason":"حذف من ملف العامل"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, profile_after) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}")), Some(&manager_token), None).await;
    assert!(profile_after["data"]["history"].as_array().unwrap().is_empty());
    let (_, other_profile) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_b}")), Some(&manager_token), None).await;
    assert_eq!(other_profile["data"]["history"].as_array().unwrap().len(), 1);
    let (_, financial_after_delete) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_a}/financial")), Some(&manager_token), None).await;
    assert_eq!(financial_after_delete["data"]["grossCommissionMilli"], 0);
    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{wash_a}/void"), Some(&manager_token), Some(json!({"reason":"طلب مكرر"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    test_app.cleanup();
}

#[tokio::test]
async fn worker_profile_wash_count_and_full_value_follow_add_edit_reassign_delete_and_restart() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_a = create_worker(&test_app.router, &manager_token, "عامل الإحصاءات أ").await;
    let worker_b = create_worker(&test_app.router, &manager_token, "عامل الإحصاءات ب").await;
    let wash_a = create_cash_wash_at(&test_app.router, &manager_token, &worker_a, "30", "2026-08-20T10:00:00Z").await;
    let wash_b = create_cash_wash_at(&test_app.router, &manager_token, &worker_a, "40", "2026-08-20T11:00:00Z").await;
    let profile_url_a = all_time_endpoint(&format!("/api/workers/{worker_a}"));
    let profile_url_b = all_time_endpoint(&format!("/api/workers/{worker_b}"));

    let (_, after_add) = request_json(&test_app.router, Method::GET, &profile_url_a, Some(&manager_token), None).await;
    assert_eq!(after_add["data"]["worker"]["washCount"], 2);
    assert_eq!(after_add["data"]["worker"]["totalWashValueMilli"], 70_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_b}"),
        Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Camry","manufactureYear":2024,
            "licensePlate":"1234 أ ب","price":"55","workerId":worker_a,
            "paymentType":"cash","occurredAt":"2026-08-20T11:00:00Z"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_price_edit) = request_json(&test_app.router, Method::GET, &profile_url_a, Some(&manager_token), None).await;
    assert_eq!(after_price_edit["data"]["worker"]["washCount"], 2);
    assert_eq!(after_price_edit["data"]["worker"]["totalWashValueMilli"], 85_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_b}"),
        Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Camry","manufactureYear":2024,
            "licensePlate":"1234 أ ب","price":"55","workerId":worker_b,
            "paymentType":"cash","occurredAt":"2026-08-20T11:00:00Z"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, worker_a_after_reassign) = request_json(&test_app.router, Method::GET, &profile_url_a, Some(&manager_token), None).await;
    let (_, worker_b_after_reassign) = request_json(&test_app.router, Method::GET, &profile_url_b, Some(&manager_token), None).await;
    assert_eq!(worker_a_after_reassign["data"]["worker"]["washCount"], 1);
    assert_eq!(worker_a_after_reassign["data"]["worker"]["totalWashValueMilli"], 30_000);
    assert_eq!(worker_b_after_reassign["data"]["worker"]["washCount"], 1);
    assert_eq!(worker_b_after_reassign["data"]["worker"]["totalWashValueMilli"], 55_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        &format!("/api/washes/{wash_a}/void"),
        Some(&manager_token),
        Some(json!({"reason":"اختبار حذف الإحصاءات"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, worker_a_after_delete) = request_json(&test_app.router, Method::GET, &profile_url_a, Some(&manager_token), None).await;
    assert_eq!(worker_a_after_delete["data"]["worker"]["washCount"], 0);
    assert_eq!(worker_a_after_delete["data"]["worker"]["totalWashValueMilli"], 0);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened = build_router(create_state(data_dir.clone()).expect("database should reopen"));
    let reopened_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, persisted_a) = request_json(&reopened, Method::GET, &profile_url_a, Some(&reopened_token), None).await;
    let (_, persisted_b) = request_json(&reopened, Method::GET, &profile_url_b, Some(&reopened_token), None).await;
    assert_eq!(persisted_a["data"]["worker"]["washCount"], 0);
    assert_eq!(persisted_a["data"]["worker"]["totalWashValueMilli"], 0);
    assert_eq!(persisted_b["data"]["worker"]["washCount"], 1);
    assert_eq!(persisted_b["data"]["worker"]["totalWashValueMilli"], 55_000);
    drop(reopened);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn worker_movements_settle_persist_delete_and_recalculate_without_negative_balance() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل تصفية المستقطعات").await;
    let ledger_url = all_time_endpoint(&format!("/api/workers/{worker_id}/withdrawals-returns"));

    let (status, withdrawal) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"withdrawal","amount":"500","occurredAt":"2026-08-10T12:00:00Z","notes":"مسحوب الاختبار"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawal_id = withdrawal["data"]["id"].as_str().unwrap().to_owned();
    let (status, returned) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"return","amount":"200","occurredAt":"2026-08-11T12:00:00Z","notes":"مرتجع الاختبار"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let return_id = returned["data"]["id"].as_str().unwrap().to_owned();

    let (status, _) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"return","amount":"301","occurredAt":"2026-08-12T12:00:00Z"})),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a return cannot make the persisted outstanding balance negative");

    let (status, settled) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns/settle"), Some(&manager_token),
        Some(json!({"occurredAt":"2026-08-13T12:00:00Z"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settled["data"]["amountMilli"], 300_000);
    let settlement_id = settled["data"]["id"].as_str().unwrap().to_owned();
    let (_, after_settlement) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_settlement["data"]["totalWithdrawalsMilli"], 500_000);
    assert_eq!(after_settlement["data"]["totalReturnsMilli"], 200_000);
    assert_eq!(after_settlement["data"]["totalSettlementsMilli"], 300_000);
    assert_eq!(after_settlement["data"]["outstandingBalanceMilli"], 0);
    assert_eq!(after_settlement["data"]["transactions"].as_array().unwrap().len(), 3);
    assert!(after_settlement["data"]["transactions"].as_array().unwrap().iter().any(|movement| movement["type"] == "settlement"));
    let (status, _) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns/settle"), Some(&manager_token),
        Some(json!({"occurredAt":"2026-08-14T12:00:00Z"})),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened = build_router(create_state(data_dir.clone()).expect("movement database should reopen"));
    let reopened_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, persisted) = request_json(&reopened, Method::GET, &ledger_url, Some(&reopened_token), None).await;
    assert_eq!(persisted["data"]["outstandingBalanceMilli"], 0);
    assert_eq!(persisted["data"]["transactions"].as_array().unwrap().len(), 3);

    let (status, _) = request_json(&reopened, Method::DELETE, &format!("/api/workers/{worker_id}/withdrawals-returns/{settlement_id}"), Some(&reopened_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, without_settlement) = request_json(&reopened, Method::GET, &ledger_url, Some(&reopened_token), None).await;
    assert_eq!(without_settlement["data"]["totalSettlementsMilli"], 0);
    assert_eq!(without_settlement["data"]["outstandingBalanceMilli"], 300_000);

    let (status, _) = request_json(&reopened, Method::DELETE, &format!("/api/workers/{worker_id}/withdrawals-returns/{return_id}"), Some(&reopened_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, without_return) = request_json(&reopened, Method::GET, &ledger_url, Some(&reopened_token), None).await;
    assert_eq!(without_return["data"]["totalReturnsMilli"], 0);
    assert_eq!(without_return["data"]["outstandingBalanceMilli"], 500_000);

    let (status, _) = request_json(&reopened, Method::DELETE, &format!("/api/workers/{worker_id}/withdrawals-returns/{withdrawal_id}"), Some(&reopened_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, empty) = request_json(&reopened, Method::GET, &ledger_url, Some(&reopened_token), None).await;
    assert_eq!(empty["data"]["totalWithdrawalsMilli"], 0);
    assert_eq!(empty["data"]["outstandingBalanceMilli"], 0);
    assert!(empty["data"]["transactions"].as_array().unwrap().is_empty());
    drop(reopened);

    let connection = Connection::open(data_dir.join("carwash.db")).unwrap();
    let retained_deleted_rows: i64 = connection.query_row(
        "SELECT COUNT(*) FROM worker_withdrawal_returns WHERE worker_id=?1 AND deleted_at IS NOT NULL",
        [worker_id],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(retained_deleted_rows, 3, "deleted movements remain persisted for auditability");
    drop(connection);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn worker_deductions_and_partial_payments_share_one_persistent_outstanding_balance() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل تسديد الاستقطاع").await;
    let ledger_url = all_time_endpoint(&format!("/api/workers/{worker_id}/withdrawals-returns"));

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/expenses",
        Some(&manager_token),
        Some(json!({
            "description":"استقطاع عامل الاختبار","category":"أخرى","amount":"300",
            "occurredAt":"2026-08-01T12:00:00Z","allocationType":"workers"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, deduction_only) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(deduction_only["data"]["totalDeductionsMilli"], 300_000);
    assert_eq!(deduction_only["data"]["outstandingBalanceMilli"], 300_000);
    assert!(deduction_only["data"]["transactions"].as_array().unwrap().iter().any(|movement| movement["type"] == "deduction" && movement["amountMilli"] == 300_000));

    let (status, first_payment) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"deduction_payment","amount":"100","occurredAt":"2026-08-02T12:00:00Z","notes":"الدفعة الأولى"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let first_payment_id = first_payment["data"]["id"].as_str().unwrap().to_owned();
    let (_, after_first_payment) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_first_payment["data"]["totalDeductionPaymentsMilli"], 100_000);
    assert_eq!(after_first_payment["data"]["outstandingBalanceMilli"], 200_000);

    let (status, second_payment) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"deduction_payment","amount":"50","occurredAt":"2026-08-03T12:00:00Z","notes":"الدفعة الثانية"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let second_payment_id = second_payment["data"]["id"].as_str().unwrap().to_owned();
    let (_, after_second_payment) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_second_payment["data"]["totalDeductionPaymentsMilli"], 150_000);
    assert_eq!(after_second_payment["data"]["outstandingBalanceMilli"], 150_000);

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/workers/{worker_id}/withdrawals-returns/{first_payment_id}"), Some(&manager_token),
        Some(json!({"transactionType":"deduction_payment","amount":"120","occurredAt":"2026-08-02T12:00:00Z","notes":"الدفعة الأولى معدلة"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_edit) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_edit["data"]["totalDeductionPaymentsMilli"], 170_000);
    assert_eq!(after_edit["data"]["outstandingBalanceMilli"], 130_000);
    let edited = after_edit["data"]["transactions"].as_array().unwrap().iter().find(|movement| movement["id"] == first_payment_id).unwrap();
    assert_eq!(edited["amountMilli"], 120_000);
    assert_eq!(edited["notes"], "الدفعة الأولى معدلة");

    let (status, _) = request_json(
        &test_app.router, Method::DELETE, &format!("/api/workers/{worker_id}/withdrawals-returns/{second_payment_id}"), Some(&manager_token), None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_payment_delete) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_payment_delete["data"]["totalDeductionPaymentsMilli"], 120_000);
    assert_eq!(after_payment_delete["data"]["outstandingBalanceMilli"], 180_000);

    let (status, _) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"withdrawal","amount":"200","occurredAt":"2026-08-04T12:00:00Z","notes":"مسحوب مع الاستقطاع"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, combined) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(combined["data"]["outstandingBalanceMilli"], 380_000);

    let (status, _) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns"), Some(&manager_token),
        Some(json!({"transactionType":"return","amount":"80","occurredAt":"2026-08-05T12:00:00Z","notes":"مرتجع مع الاستقطاع"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_return) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_return["data"]["outstandingBalanceMilli"], 300_000);

    let (status, settled) = request_json(
        &test_app.router, Method::POST, &format!("/api/workers/{worker_id}/withdrawals-returns/settle"), Some(&manager_token),
        Some(json!({"occurredAt":"2026-08-06T12:00:00Z"})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settled["data"]["amountMilli"], 300_000);
    let (_, after_settlement) = request_json(&test_app.router, Method::GET, &ledger_url, Some(&manager_token), None).await;
    assert_eq!(after_settlement["data"]["outstandingBalanceMilli"], 0);
    for movement_type in ["deduction", "deduction_payment", "withdrawal", "return", "settlement"] {
        assert!(after_settlement["data"]["transactions"].as_array().unwrap().iter().any(|movement| movement["type"] == movement_type));
    }

    let (_, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")), Some(&manager_token), None).await;
    assert_eq!(financial["data"]["deductionsMilli"], 300_000);
    assert_eq!(financial["data"]["grossCommissionMilli"], 0);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened = build_router(create_state(data_dir.clone()).expect("deduction payment database should reopen"));
    let reopened_token = login(&reopened, "manager.test", MANAGER_PASSWORD).await;
    let (_, persisted) = request_json(&reopened, Method::GET, &ledger_url, Some(&reopened_token), None).await;
    assert_eq!(persisted["data"]["totalDeductionsMilli"], 300_000);
    assert_eq!(persisted["data"]["totalDeductionPaymentsMilli"], 120_000);
    assert_eq!(persisted["data"]["totalSettlementsMilli"], 300_000);
    assert_eq!(persisted["data"]["outstandingBalanceMilli"], 0);
    assert_eq!(persisted["data"]["transactions"].as_array().unwrap().len(), 5);
    drop(reopened);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn shared_expense_is_split_equally_and_auditable_per_worker() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let first_worker = create_worker(&test_app.router, &manager_token, "العامل الأول").await;
    let second_worker = create_worker(&test_app.router, &manager_token, "العامل الثاني").await;

    let (status, payload) = request_json(
        &test_app.router,
        Method::POST,
        "/api/expenses",
        Some(&manager_token),
        Some(json!({
            "description": "مواد التنظيف",
            "category": "materials",
            "amount": "1000",
            "occurredAt": "2026-08-29T12:00:00Z",
            "allocationType": "shared",
            "businessBps": 5000,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["data"]["businessAmountMilli"], 500_000);
    assert_eq!(payload["data"]["workersAmountMilli"], 500_000);
    assert_eq!(payload["data"]["workerCount"], 2);

    for worker_id in [&first_worker, &second_worker] {
        let (status, payload) = request_json(
            &test_app.router,
            Method::GET,
            &all_time_endpoint(&format!("/api/workers/{worker_id}/financial")),
            Some(&manager_token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["data"]["deductionsMilli"], 250_000);
    }

    test_app.cleanup();
}

#[tokio::test]
async fn expense_deletion_removes_only_the_selected_expense_and_reverses_it() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    create_worker(&test_app.router, &manager_token, "عامل حذف المصروف").await;
    let mut ids = Vec::new();
    for description in ["مصروف سيحذف", "مصروف سيبقى"] {
        let (status, payload) = request_json(
            &test_app.router, Method::POST, "/api/expenses", Some(&manager_token),
            Some(json!({ "description":description,"category":"اختبار","amount":"40","occurredAt":"2026-08-29T12:00:00Z","allocationType":"shared","businessBps":5000 })),
        ).await;
        assert_eq!(status, StatusCode::OK);
        ids.push(payload["data"]["id"].as_str().unwrap().to_owned());
    }
    let (status, payload) = request_json(&test_app.router, Method::DELETE, &format!("/api/expenses/{}", ids[0]), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK, "delete response: {payload}");
    assert_eq!(payload["data"]["deleted"], true);
    let (deleted_status, _) = request_json(&test_app.router, Method::GET, &format!("/api/expenses/{}", ids[0]), Some(&manager_token), None).await;
    let (kept_status, _) = request_json(&test_app.router, Method::GET, &format!("/api/expenses/{}", ids[1]), Some(&manager_token), None).await;
    assert_eq!(deleted_status, StatusCode::NOT_FOUND);
    assert_eq!(kept_status, StatusCode::OK);
    test_app.cleanup();
}

#[tokio::test]
async fn showroom_statistics_use_wash_date_and_original_payment_type() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل الإحصاءات").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض الإحصاءات","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap();
    for occurred_at in ["2026-08-01T09:00:00Z", "2026-08-29T10:00:00Z", "2026-08-29T18:00:00Z"] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
            Some(json!({"vehicleMake":"Test","vehicleModel":"Car","price":"20","workerId":worker_id,"paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"bank","occurredAt":occurred_at,"clientRequestId":Uuid::new_v4().to_string()})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, washes) = request_json(&test_app.router, Method::GET, "/api/washes", Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(washes["data"]["items"].as_array().unwrap().iter().all(|wash| wash["showroomPaymentMethod"] == "bank"));
    let (status, _) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({"vehicleMake":"Invalid","vehicleModel":"Hidden value","price":"20","workerId":worker_id,"paymentType":"cash","showroomPaymentMethod":"bank","occurredAt":"2026-08-29T20:00:00Z","clientRequestId":Uuid::new_v4().to_string()})),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "normal customers must not store hidden showroom payment values");
    let endpoint = |from: &str, to: &str, payment: &str| format!(
        "/api/showrooms/{showroom_id}/statistics?from={from}&to={to}&paymentType={payment}"
    );
    let (_, all) = request_json(&test_app.router, Method::GET, &endpoint("2026-08-01T00:00:00Z", "2026-08-29T23:59:59Z", "all"), Some(&manager_token), None).await;
    let (_, single_day) = request_json(&test_app.router, Method::GET, &endpoint("2026-08-29T00:00:00Z", "2026-08-29T23:59:59Z", "debt"), Some(&manager_token), None).await;
    let (_, cash) = request_json(&test_app.router, Method::GET, &endpoint("2026-08-01T00:00:00Z", "2026-08-29T23:59:59Z", "cash"), Some(&manager_token), None).await;
    assert_eq!(all["data"]["carCount"], 3);
    assert_eq!(single_day["data"]["carCount"], 2);
    assert_eq!(cash["data"]["carCount"], 0, "debt transactions must never be reclassified as cash after payment");
    test_app.cleanup();
}

#[tokio::test]
async fn showroom_debts_follow_source_washes_without_duplicates_and_persist() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل ديون المعارض").await;
    let (status, showroom) = request_json(
        &test_app.router, Method::POST, "/api/showrooms", Some(&manager_token),
        Some(json!({"name":"معرض الديون المتكامل","contactName":"مسؤول المعرض","phone":"0910000000","notes":"طرابلس","isActive":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let showroom_id = showroom["data"]["id"].as_str().unwrap().to_owned();
    let request_id = Uuid::new_v4().to_string();
    let original_wash = json!({
        "vehicleMake":"Toyota","vehicleModel":"Camry","manufactureYear":2025,
        "licensePlate":"1111 د ي","carColor":"أبيض","price":"100","workerId":worker_id,
        "paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"bank",
        "occurredAt":"2026-08-10T11:30:00Z","clientRequestId":request_id
    });
    let (status, created) = request_json(&test_app.router, Method::POST, "/api/washes", Some(&manager_token), Some(original_wash.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["data"]["duplicate"], false);
    let wash_id = created["data"]["id"].as_str().unwrap().to_owned();
    let (status, duplicate) = request_json(&test_app.router, Method::POST, "/api/washes", Some(&manager_token), Some(original_wash)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(duplicate["data"]["duplicate"], true);
    assert_eq!(duplicate["data"]["id"], wash_id);

    let (status, debts) = request_json(&test_app.router, Method::GET, &all_time_endpoint("/api/showroom-debts"), Some(&manager_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(debts["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(debts["data"]["items"][0]["showroom"]["id"], showroom_id);
    assert_eq!(debts["data"]["items"][0]["outstandingWashCount"], 1);
    assert_eq!(debts["data"]["items"][0]["totalOutstandingMilli"], 100_000);

    let august_endpoint = format!("/api/showroom-debts/{showroom_id}?from=2026-08-01T00:00:00Z&to=2026-08-31T23:59:59Z");
    let (_, august) = request_json(&test_app.router, Method::GET, &august_endpoint, Some(&manager_token), None).await;
    assert!(august["data"]["showroom"]["createdAt"].as_str().is_some());
    assert_eq!(august["data"]["showroom"]["phone"], "0910000000");
    assert_eq!(august["data"]["showroom"]["notes"], "طرابلس");
    assert_eq!(august["data"]["outstandingWashCount"], 1);
    assert_eq!(august["data"]["totalOutstandingMilli"], 100_000);
    assert_eq!(august["data"]["operations"][0]["id"], wash_id);
    assert_eq!(august["data"]["operations"][0]["vehicleMake"], "Toyota");
    assert_eq!(august["data"]["operations"][0]["vehicleModel"], "Camry");
    assert_eq!(august["data"]["operations"][0]["licensePlate"], "1111 د ي");
    assert_eq!(august["data"]["operations"][0]["carColor"], "أبيض");
    assert_eq!(august["data"]["operations"][0]["worker"]["id"], worker_id);

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{wash_id}"), Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Land Cruiser","manufactureYear":2025,
            "licensePlate":"1111 د ي","carColor":"أسود","price":"125","workerId":worker_id,
            "paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"bank",
            "occurredAt":"2026-08-10T11:30:00Z"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_edit) = request_json(&test_app.router, Method::GET, &august_endpoint, Some(&manager_token), None).await;
    assert_eq!(after_edit["data"]["outstandingWashCount"], 1);
    assert_eq!(after_edit["data"]["totalOutstandingMilli"], 125_000);
    assert_eq!(after_edit["data"]["operations"][0]["vehicleModel"], "Land Cruiser");
    assert_eq!(after_edit["data"]["operations"][0]["carColor"], "أسود");

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{wash_id}"), Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Land Cruiser","manufactureYear":2025,
            "licensePlate":"1111 د ي","carColor":"أسود","price":"","workerId":worker_id,
            "paymentType":"cash","occurredAt":"2026-08-10T11:30:00Z"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, removed_from_debt) = request_json(&test_app.router, Method::GET, &all_time_endpoint("/api/showroom-debts"), Some(&manager_token), None).await;
    assert!(removed_from_debt["data"]["items"].as_array().unwrap().is_empty());

    let (status, _) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{wash_id}"), Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Land Cruiser","manufactureYear":2025,
            "licensePlate":"1111 د ي","carColor":"أسود","price":"","workerId":worker_id,
            "paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"cash",
            "occurredAt":"2026-08-10T11:30:00Z"
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (status, september_wash) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({
            "vehicleMake":"Kia","vehicleModel":"Sportage","licensePlate":"2222 د ي","price":"50","workerId":worker_id,
            "paymentType":"showroom","showroomId":showroom_id,"showroomPaymentMethod":"cash",
            "occurredAt":"2026-09-02T09:00:00Z","clientRequestId":Uuid::new_v4().to_string()
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let september_wash_id = september_wash["data"]["id"].as_str().unwrap();
    let (_, all_debts) = request_json(&test_app.router, Method::GET, &all_time_endpoint("/api/showroom-debts"), Some(&manager_token), None).await;
    assert_eq!(all_debts["data"]["items"][0]["outstandingWashCount"], 2);
    assert_eq!(all_debts["data"]["items"][0]["totalOutstandingMilli"], 175_000);
    let (_, august_after_second) = request_json(&test_app.router, Method::GET, &august_endpoint, Some(&manager_token), None).await;
    assert_eq!(august_after_second["data"]["outstandingWashCount"], 1);
    assert_eq!(august_after_second["data"]["totalOutstandingMilli"], 125_000);
    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{september_wash_id}/void"), Some(&manager_token), Some(json!({"reason":"إلغاء اختبار الدين"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_cancel) = request_json(&test_app.router, Method::GET, &all_time_endpoint("/api/showroom-debts"), Some(&manager_token), None).await;
    assert_eq!(after_cancel["data"]["items"][0]["outstandingWashCount"], 1);
    assert_eq!(after_cancel["data"]["items"][0]["totalOutstandingMilli"], 125_000);

    let TestApp { router, data_dir } = test_app;
    drop(router);
    let reopened_router = build_router(create_state(data_dir.clone()).expect("database should reopen"));
    let reopened_token = login(&reopened_router, "manager.test", MANAGER_PASSWORD).await;
    let (_, persisted) = request_json(&reopened_router, Method::GET, &august_endpoint, Some(&reopened_token), None).await;
    assert_eq!(persisted["data"]["outstandingWashCount"], 1);
    assert_eq!(persisted["data"]["totalOutstandingMilli"], 125_000);
    assert_eq!(persisted["data"]["operations"][0]["id"], wash_id);
    drop(reopened_router);
    let _ = fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn manager_edits_and_reversals_preserve_financial_integrity() {
    let test_app = TestApp::new();
    let token = bootstrap_manager(&test_app.router).await;
    let first = create_worker(&test_app.router, &token, "عامل أول").await;
    let second = create_worker(&test_app.router, &token, "عامل ثان").await;
    let wash = create_cash_wash(&test_app.router, &token, &first, "50").await;

    let (status, _) = request_json(&test_app.router, Method::PATCH, &format!("/api/washes/{wash}"), Some(&token), Some(json!({
        "vehicleMake":"Toyota","vehicleModel":"Camry","manufactureYear":2024,"licensePlate":"1234 أ ب","price":"80","workerId":first,"paymentType":"cash","occurredAt":"2026-08-29T10:00:00Z"
    }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{first}/financial")), Some(&token), None).await;
    assert_eq!(financial["data"]["grossCommissionMilli"], 40_000);

    let (status, created) = request_json(&test_app.router, Method::POST, "/api/expenses", Some(&token), Some(json!({
        "description":"مواد مشتركة","category":"أخرى","amount":"1000","occurredAt":"2026-08-29T12:00:00Z","allocationType":"shared","businessBps":5000
    }))).await;
    assert_eq!(status, StatusCode::OK);
    let expense = created["data"]["id"].as_str().unwrap().to_owned();
    let _third = create_worker(&test_app.router, &token, "عامل أضيف لاحقًا").await;
    let (status, _) = request_json(&test_app.router, Method::PATCH, &format!("/api/expenses/{expense}"), Some(&token), Some(json!({
        "description":"مواد مشتركة معدلة","category":"أخرى","amount":"1200","occurredAt":"2026-08-29T12:00:00Z","allocationType":"shared"
    }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, detail) = request_json(&test_app.router, Method::GET, &format!("/api/expenses/{expense}"), Some(&token), None).await;
    assert_eq!(detail["data"]["allocations"].as_array().unwrap().len(), 2, "editing must retain the original worker snapshot");
    for worker in [&first, &second] {
        let (_, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{worker}/financial")), Some(&token), None).await;
        assert_eq!(financial["data"]["deductionsMilli"], 300_000);
    }
    let (status, _) = request_json(&test_app.router, Method::DELETE, &format!("/api/expenses/{expense}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{first}/financial")), Some(&token), None).await;
    assert_eq!(financial["data"]["deductionsMilli"], 0);

    let (status, _) = request_json(&test_app.router, Method::POST, &format!("/api/washes/{wash}/void"), Some(&token), Some(json!({"reason":"اختبار العكس"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, financial) = request_json(&test_app.router, Method::GET, &all_time_endpoint(&format!("/api/workers/{first}/financial")), Some(&token), None).await;
    assert_eq!(financial["data"]["grossCommissionMilli"], 0);

    test_app.cleanup();
}

#[tokio::test]
async fn overnight_completion_preserves_original_operation_and_financial_date() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل دورة المبيت").await;
    let business_today = (Utc::now() + Duration::hours(2)).date_naive();
    let operation_day = business_today - Duration::days(2);
    let following_day = operation_day + Duration::days(1);
    let operation_key = operation_day.format("%Y-%m-%d").to_string();
    let following_key = following_day.format("%Y-%m-%d").to_string();
    let operation_start_utc = operation_day.and_hms_opt(0, 0, 0).unwrap() - Duration::hours(2);
    let operation_at = format!("{}Z", (operation_start_utc + Duration::hours(10)).format("%Y-%m-%dT%H:%M:%S"));

    let (status, created) = request_json(
        &test_app.router, Method::POST, "/api/washes", Some(&manager_token),
        Some(json!({
            "vehicleMake":"Toyota","vehicleModel":"Overnight Original","licensePlate":"OVERNIGHT-1",
            "carColor":"أبيض","washType":"غسيل كامل","price":"50","workerId":worker_id,
            "paymentType":"cash","occurredAt":operation_at,"clientRequestId":Uuid::new_v4().to_string()
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let wash_id = created["data"]["wash"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["data"]["wash"]["commissionMilli"], 25_000);

    let selected_query = format!("date={operation_key}");
    let (_, recent_before) = request_json(&test_app.router, Method::GET, &format!("/api/washes?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(recent_before["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(recent_before["data"]["items"][0]["id"], wash_id);

    let connection = Connection::open(test_app.data_dir.join("carwash.db")).unwrap();
    let transactions_before: i64 = connection.query_row(
        "SELECT COUNT(*) FROM financial_transactions WHERE source_id=?1", [&wash_id], |row| row.get(0),
    ).unwrap();
    drop(connection);
    assert_eq!(transactions_before, 1);

    let (status, marked) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{wash_id}/overnight"), Some(&manager_token),
        Some(json!({"isOvernight":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(marked["data"]["wash"]["id"], wash_id);
    assert_eq!(marked["data"]["wash"]["occurredAt"], operation_at);

    let (_, recent_after_mark) = request_json(&test_app.router, Method::GET, &format!("/api/washes?{selected_query}"), Some(&manager_token), None).await;
    assert!(recent_after_mark["data"]["items"].as_array().unwrap().is_empty());
    let (_, dashboard_after_mark) = request_json(&test_app.router, Method::GET, &format!("/api/dashboard?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(dashboard_after_mark["data"]["todayWashes"], 1);
    assert!(dashboard_after_mark["data"]["recentWashes"].as_array().unwrap().is_empty());
    let (_, overnight) = request_json(&test_app.router, Method::GET, &format!("/api/overnight-cars?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(overnight["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(overnight["data"]["items"][0]["wash"]["id"], wash_id);
    assert_eq!(overnight["data"]["items"][0]["wash"]["occurredAt"], operation_at);

    let (status, completed) = request_json(
        &test_app.router, Method::PATCH, &format!("/api/washes/{wash_id}/paid?{selected_query}"), Some(&manager_token),
        Some(json!({"isPaid":true})),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(completed["data"]["wash"]["id"], wash_id);
    assert_eq!(completed["data"]["wash"]["occurredAt"], operation_at);
    assert_eq!(completed["data"]["settlementMilli"], 50_000);

    let (_, active_overnight_after_paid) = request_json(&test_app.router, Method::GET, &format!("/api/overnight-cars?{selected_query}"), Some(&manager_token), None).await;
    assert!(active_overnight_after_paid["data"]["items"].as_array().unwrap().is_empty());
    let (_, paid_original_day) = request_json(&test_app.router, Method::GET, &format!("/api/paid-cars?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(paid_original_day["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(paid_original_day["data"]["items"][0]["id"], wash_id);
    assert_eq!(paid_original_day["data"]["items"][0]["occurredAt"], operation_at);
    assert_eq!(paid_original_day["data"]["settlementMilli"], 50_000);
    let (_, paid_following_day) = request_json(&test_app.router, Method::GET, &format!("/api/paid-cars?date={following_key}"), Some(&manager_token), None).await;
    assert!(paid_following_day["data"]["items"].as_array().unwrap().is_empty());
    assert_eq!(paid_following_day["data"]["settlementMilli"], 0);

    let (_, finance_original_day) = request_json(&test_app.router, Method::GET, &format!("/api/finance/overview?{selected_query}"), Some(&manager_token), None).await;
    assert_eq!(finance_original_day["data"]["totalWashRevenueMilli"], 50_000);
    assert_eq!(finance_original_day["data"]["workerCommissionsMilli"], 25_000);
    assert_eq!(finance_original_day["data"]["netProfitBeforeExpensesMilli"], 25_000);
    let (_, finance_following_day) = request_json(&test_app.router, Method::GET, &format!("/api/finance/overview?date={following_key}"), Some(&manager_token), None).await;
    assert_eq!(finance_following_day["data"]["totalWashRevenueMilli"], 0);
    assert_eq!(finance_following_day["data"]["workerCommissionsMilli"], 0);

    let connection = Connection::open(test_app.data_dir.join("carwash.db")).unwrap();
    let stored: (String, i64, i64, Option<String>, Option<String>) = connection.query_row(
        "SELECT occurred_at,commission_milli,is_paid,paid_at,paid_by FROM wash_operations WHERE id=?1",
        [&wash_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(stored.0, operation_at);
    assert_eq!(stored.1, 25_000);
    assert_eq!(stored.2, 1);
    assert!(stored.3.is_some(), "paidAt remains available as informational metadata");
    assert!(stored.4.is_none(), "completion must not be financially associated with the clicking account");
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM wash_operations WHERE id=?1", [&wash_id], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM overnight_cars WHERE wash_id=?1", [&wash_id], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM financial_transactions WHERE source_id=?1", [&wash_id], |row| row.get::<_,i64>(0)).unwrap(), transactions_before);
    drop(connection);
    test_app.cleanup();
}

#[tokio::test]
async fn finance_separates_profit_before_and_after_business_expenses_by_period() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل اختبار الربحين").await;
    let business_today = (Utc::now() + Duration::hours(2)).date_naive();
    let selected = business_today - Duration::days(2);
    let unrelated = selected + Duration::days(1);
    let selected_key = selected.format("%Y-%m-%d").to_string();
    let unrelated_key = unrelated.format("%Y-%m-%d").to_string();
    let selected_start_utc = selected.and_hms_opt(0, 0, 0).unwrap() - Duration::hours(2);
    let unrelated_start_utc = unrelated.and_hms_opt(0, 0, 0).unwrap() - Duration::hours(2);
    let wash_at = format!("{}Z", (selected_start_utc + Duration::hours(9)).format("%Y-%m-%dT%H:%M:%S"));
    let selected_expense_at = format!("{}Z", (selected_start_utc + Duration::hours(12)).format("%Y-%m-%dT%H:%M:%S"));
    let unrelated_expense_at = format!("{}Z", (unrelated_start_utc + Duration::hours(12)).format("%Y-%m-%dT%H:%M:%S"));
    create_cash_wash_at(&test_app.router, &manager_token, &worker_id, "50", &wash_at).await;
    for (description, amount, occurred_at) in [
        ("مصروف الأعمال المحدد", "500", &selected_expense_at),
        ("مصروف يوم آخر", "100", &unrelated_expense_at),
    ] {
        let (status, _) = request_json(
            &test_app.router, Method::POST, "/api/expenses", Some(&manager_token),
            Some(json!({"description":description,"category":"اختبار","amount":amount,"occurredAt":occurred_at,"allocationType":"business"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (_, selected_finance) = request_json(&test_app.router, Method::GET, &format!("/api/finance/overview?date={selected_key}"), Some(&manager_token), None).await;
    assert_eq!(selected_finance["data"]["totalWashRevenueMilli"], 50_000);
    assert_eq!(selected_finance["data"]["workerCommissionsMilli"], 25_000);
    assert_eq!(selected_finance["data"]["businessExpensesMilli"], 500_000);
    assert_eq!(selected_finance["data"]["netProfitBeforeExpensesMilli"], 25_000);
    assert_eq!(selected_finance["data"]["netProfitAfterExpensesMilli"], -475_000);

    let (_, unrelated_finance) = request_json(&test_app.router, Method::GET, &format!("/api/finance/overview?date={unrelated_key}"), Some(&manager_token), None).await;
    assert_eq!(unrelated_finance["data"]["netProfitBeforeExpensesMilli"], 0);
    assert_eq!(unrelated_finance["data"]["netProfitAfterExpensesMilli"], -100_000);

    let range_endpoint = format!(
        "/api/finance/overview?from={}Z&to={}Z",
        selected_start_utc.format("%Y-%m-%dT%H:%M:%S"),
        (unrelated_start_utc + Duration::hours(24) - Duration::seconds(1)).format("%Y-%m-%dT%H:%M:%S")
    );
    let (_, range_finance) = request_json(&test_app.router, Method::GET, &range_endpoint, Some(&manager_token), None).await;
    assert_eq!(range_finance["data"]["netProfitBeforeExpensesMilli"], 25_000);
    assert_eq!(range_finance["data"]["netProfitAfterExpensesMilli"], -575_000);
    test_app.cleanup();
}

#[tokio::test]
async fn overnight_cars_are_unique_linked_and_manager_only() {
    let test_app = TestApp::new();
    let manager_token = bootstrap_manager(&test_app.router).await;
    let worker_id = create_worker(&test_app.router, &manager_token, "عامل المبيت").await;
    let wash_id = create_cash_wash(&test_app.router, &manager_token, &worker_id, "80").await;

    let (status, _) = request_json(
        &test_app.router,
        Method::POST,
        "/api/users",
        Some(&manager_token),
        Some(json!({
            "fullName":"موظف التشغيل",
            "username":"overnight.employee",
            "password":EMPLOYEE_PASSWORD,
            "roleCode":"employee",
            "isActive":true
        })),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let employee_token = login(&test_app.router, "overnight.employee", EMPLOYEE_PASSWORD).await;
    let employee_wash_id = create_employee_cash_wash(&test_app.router, &employee_token, &worker_id, "5678 أ ب").await;
    let overnight_endpoint = all_time_endpoint("/api/overnight-cars");

    let edit_payload = json!({
        "vehicleMake":"Toyota",
        "vehicleModel":"Camry",
        "manufactureYear":2024,
        "licensePlate":"1234 أ ب",
        "carColor":"أزرق",
        "price":"",
        "workerId":worker_id,
        "paymentType":"cash",
        "occurredAt":"2026-08-29T10:00:00Z",
        "markAsOvernight":true
    });
    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_id}"),
        Some(&employee_token),
        Some(edit_payload.clone()),
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request_json(
        &test_app.router,
        Method::GET,
        &overnight_endpoint,
        Some(&employee_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);

    let employee_edit_payload = json!({
        "vehicleMake":"Honda",
        "vehicleModel":"Civic",
        "manufactureYear":2023,
        "licensePlate":"5678 أ ب",
        "carColor":"أبيض",
        "price":"",
        "workerId":worker_id,
        "paymentType":"cash",
        "occurredAt":"2026-08-29T09:00:00Z",
        "markAsOvernight":true
    });
    let (status, employee_update) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{employee_wash_id}"),
        Some(&employee_token),
        Some(employee_edit_payload),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(employee_update["data"]["wash"]["worker"]["id"], worker_id);
    assert_eq!(employee_update["data"]["wash"]["priceMilli"], 40_000);
    assert!(employee_update["data"]["wash"].get("commissionMilli").is_none());
    let (_, employee_overnight) = request_json(&test_app.router, Method::GET, &overnight_endpoint, Some(&employee_token), None).await;
    assert_eq!(employee_overnight["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(employee_overnight["data"]["items"][0]["wash"]["id"], employee_wash_id);
    assert_eq!(employee_overnight["data"]["items"][0]["wash"]["priceMilli"], 40_000);
    assert!(employee_overnight["data"]["items"][0]["wash"].get("commissionMilli").is_none());

    for _ in 0..2 {
        let (status, updated) = request_json(
            &test_app.router,
            Method::PATCH,
            &format!("/api/washes/{wash_id}"),
            Some(&manager_token),
            Some(edit_payload.clone()),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["data"]["wash"]["worker"]["id"], worker_id);
        assert_eq!(updated["data"]["wash"]["priceMilli"], 80_000);
        assert_eq!(updated["data"]["wash"]["commissionMilli"], 40_000);
    }

    let mut unmark_payload = edit_payload.clone();
    unmark_payload["markAsOvernight"] = json!(false);
    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_id}"),
        Some(&manager_token),
        Some(unmark_payload),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, after_unmark) = request_json(&test_app.router, Method::GET, &overnight_endpoint, Some(&manager_token), None).await;
    assert_eq!(after_unmark["data"]["items"].as_array().unwrap().len(), 1);

    let (status, _) = request_json(
        &test_app.router,
        Method::PATCH,
        &format!("/api/washes/{wash_id}"),
        Some(&manager_token),
        Some(edit_payload.clone()),
    ).await;
    assert_eq!(status, StatusCode::OK);

    let (status, overnight) = request_json(
        &test_app.router,
        Method::GET,
        &overnight_endpoint,
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    let items = overnight["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the same wash must not create duplicate overnight records");
    let manager_item = items.iter().find(|item| item["wash"]["id"] == wash_id).unwrap();
    assert_eq!(manager_item["wash"]["vehicleMake"], "Toyota");
    assert_eq!(manager_item["wash"]["priceMilli"], 80_000);
    assert_eq!(manager_item["wash"]["carColor"], "أزرق");
    assert_eq!(manager_item["wash"]["worker"]["id"], worker_id);
    assert_eq!(manager_item["wash"]["commissionMilli"], 40_000);

    let (status, _) = request_json(
        &test_app.router,
        Method::DELETE,
        &format!("/api/overnight-cars/{}", manager_item["id"].as_str().unwrap()),
        Some(&employee_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, washes) = request_json(
        &test_app.router,
        Method::GET,
        "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z",
        Some(&manager_token),
        None,
    ).await;
    assert!(!washes["data"]["items"].as_array().unwrap().iter().any(|wash| wash["id"] == wash_id), "overnight washes must leave Recent Operations");

    let (status, _) = request_json(
        &test_app.router,
        Method::DELETE,
        &format!("/api/overnight-cars/{}", manager_item["id"].as_str().unwrap()),
        Some(&manager_token),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    let (_, overnight_after_delete) = request_json(&test_app.router, Method::GET, &overnight_endpoint, Some(&manager_token), None).await;
    assert_eq!(overnight_after_delete["data"]["items"].as_array().unwrap().len(), 1);
    let (_, wash_after_delete) = request_json(&test_app.router, Method::GET, "/api/washes?from=2026-08-29T00:00:00Z&to=2026-08-29T23:59:59Z", Some(&manager_token), None).await;
    assert_eq!(wash_after_delete["data"]["items"][0]["id"], wash_id);
    assert_eq!(wash_after_delete["data"]["items"][0]["carColor"], "أزرق");

    test_app.cleanup();
}
