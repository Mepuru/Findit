//! AI 能力层：服务配置、HTTP 客户端、一句话意图解析、意图应用、向量回填。
//!
//! 设计原则：
//! - 数据库操作一律接收 `&Connection`，可用内存库做单元测试；
//! - 网络部分由 [`client::AiTransport`] trait 隔离，测试用 mock 实现；
//! - prompt 构造、响应解析、容错修复、错误分类均为纯函数。

pub mod apply;
pub mod client;
pub mod config;
pub mod embed;
pub mod keystore;
pub mod parse;
