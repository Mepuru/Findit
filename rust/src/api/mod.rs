//! FRB 桥接层：只做转发，不含业务逻辑。
//!
//! 所有函数均为 `async`，FRB 会在专用线程池上执行，
//! 不会阻塞 Dart/平台线程。错误以 `FinditError` 返回，
//! FRB 将其映射为 Dart 端可区分的异常。

pub mod boxes;
pub mod categories;
pub mod db;
pub mod items;
pub mod model;
pub mod photos;
pub mod search;
pub mod settings;
pub mod simple;
pub mod units;
