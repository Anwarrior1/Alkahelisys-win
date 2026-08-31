use alkaheli_car_wash_erp_lib::{create_state, db::default_data_dir, serve};

#[tokio::main]
async fn main() {
    let data_dir = default_data_dir();
    let state = create_state(data_dir).expect("تعذر تهيئة قاعدة بيانات مركز الكحيلي");
    println!("مركز الكحيلي: الخدمة المحلية تعمل على http://127.0.0.1:8787");
    serve(state).await.expect("توقفت الخدمة المحلية");
}
