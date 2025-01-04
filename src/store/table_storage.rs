use crate::api::APIError;
use crate::{
    models::{self, *},
    trace_handler,
};
use actix::prelude::*;
use azure_data_tables::prelude::*;
use azure_storage::StorageCredentials;
use futures::{Future, StreamExt};
use rand::seq::IteratorRandom;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::{fmt::Debug, pin::Pin, sync::Arc};
use tracing_batteries::prelude::*;

type TableReference = Arc<TableClient>;

pub struct TableStorage {
    started_at: chrono::DateTime<chrono::Utc>,
    pages: TableReference,
}

impl TableStorage {
    pub fn new() -> Self {
        let connection_string = std::env::var("TABLE_STORAGE_CONNECTION_STRING").expect("Set the TABLE_STORAGE_CONNECTION_STRING environment variable before starting the server.");

        let creds = azure_storage::ConnectionString::new(&connection_string)
            .expect("a valid connection string");

        let table_service = TableServiceClient::new(
            creds
                .account_name
                .expect("The connection string must include the account name."),
            StorageCredentials::access_key(
                creds
                    .account_name
                    .expect("The connection string must include the account name.")
                    .to_string(),
                creds
                    .account_key
                    .expect("The connection string must include the account key.")
                    .to_string(),
            ),
        );

        let pages_table = table_service.table_client("pages");

        Self {
            started_at: chrono::Utc::now(),
            pages: TableReference::new(pages_table),
        }
    }

    #[tracing::instrument(err, skip(table, not_found_err), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "GET"))]
    async fn get_single<ST, T>(
        table: TableReference,
        type_name: &str,
        partition_key: String,
        row_key: String,
        not_found_err: APIError,
    ) -> Result<T, APIError>
    where
        ST: DeserializeOwned + Clone + Sync + Send,
        T: From<ST>,
    {
        let result: ST = table
            .partition_key_client(partition_key)
            .entity_client(row_key)
            .get()
            .into_future()
            .await
            .map_err(|err| {
                error!("Failed to retrieve item from table storage: {}", err);
                not_found_err
            })?
            .entity;

        Ok(result.into())
    }

    #[tracing::instrument(err, skip(table, filter), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "LIST", db.statement = %query))]
    async fn get_all_entities<ST, P>(
        table: TableReference,
        _type_name: &str,
        query: String,
        filter: P,
    ) -> Result<Vec<ST>, APIError>
    where
        ST: Serialize + DeserializeOwned + Clone + Sync + Send,
        P: Fn(&ST) -> bool,
    {
        let mut entries: Vec<ST> = vec![];

        let mut query_operation = table.query();
        if !query.is_empty() {
            query_operation = query_operation.filter(query.clone());
        }

        let mut stream = Box::pin(query_operation.into_stream::<ST>());

        while let Some(result) = stream.next().instrument(
            info_span!("get_all_entities.get_page", "otel.kind" = "client", "db.system" = "TABLESTORAGE", "db.operation" = "LIST.PAGE", db.statement = %query)
        ).await {
            let mut result = result
            .map_err(|err| {
                error!("Failed to retrieve items from table storage: {}", err);
                APIError::new(500, "Internal Server Error", "We were unable to retrieve the items you requested, this failure has been reported.")
            })?;
            entries.append(&mut result.entities);
        }

        Ok(entries.iter().filter(|&e| filter(e)).cloned().collect())
    }

    #[tracing::instrument(err, skip(table, filter), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "LIST", db.statement = %query))]
    async fn get_all<ST, T, P>(
        table: TableReference,
        type_name: &str,
        query: String,
        filter: P,
    ) -> Result<Vec<T>, APIError>
    where
        ST: Serialize + DeserializeOwned + Clone + Sync + Send,
        P: Fn(&ST) -> bool,
        T: From<ST>,
    {
        let entries: Vec<ST> =
            TableStorage::get_all_entities(table, type_name, query, filter).await?;
        Ok(entries.iter().map(|e| e.clone().into()).collect())
    }

    #[tracing::instrument(err, skip(table, item), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "PUT"))]
    async fn store_single<ST, T, PK, RK>(
        table: TableReference,
        type_name: &str,
        partition_key: PK,
        row_key: RK,
        item: ST,
    ) -> Result<T, APIError>
    where
        ST: Serialize + DeserializeOwned + Clone + Debug + Sync + Send,
        T: From<ST>,
        PK: AsRef<str> + Debug,
        RK: AsRef<str> + Debug,
    {
        table
            .partition_key_client(partition_key.as_ref())
            .entity_client(row_key.as_ref())
            .insert_or_replace(&item)?
            .into_future()
            .await
            .map_err(|err| {
                error!("Failed to store item in table storage: {}", err);
                APIError::new(503, "Service Unavailable", "We were unable to store the item you requested, this failure has been reported.")
            })?;

        Ok(item.into())
    }

    #[tracing::instrument(err, skip(table, default, updater), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "UPDATE"))]
    async fn update_single<ST, T, U>(
        table: TableReference,
        type_name: &str,
        partition_key: String,
        row_key: String,
        default: ST,
        updater: U,
    ) -> Result<T, APIError>
    where
        ST: Serialize + DeserializeOwned + Clone + Sync + Send,
        T: From<ST>,
        U: Fn(&mut ST) -> (),
    {
        let entity_client = table
            .partition_key_client(partition_key)
            .entity_client(row_key);

        let mut entity: ST = entity_client
            .get()
            .into_future()
            .await
            .map(|r| r.entity)
            .unwrap_or(default);

        updater(&mut entity);

        entity_client.insert_or_replace(&entity)?.into_future().await.map_err(|err| {
            error!("Failed to update item in table storage: {}", err);
            APIError::new(
                503,
                "Service Unavailable",
                "We were unable to update the item you requested, this failure has been reported.",
            )
        })?;

        Ok(entity.into())
    }

    #[tracing::instrument(err, skip( table), fields(otel.kind = "client", db.system = "TABLESTORAGE", db.operation = "DELETE"))]
    async fn remove_single(
        table: TableReference,
        type_name: &str,
        partition_key: u128,
        row_key: u128,
    ) -> Result<(), APIError> {
        let entity_client = table
            .partition_key_client(format!("{:0>32x}", partition_key))
            .entity_client(format!("{:0>32x}", row_key));

        entity_client.delete().into_future().await.map_err(|err| {
            error!("Failed to remove item from table storage: {}", err);
            APIError::new(
                503,
                "Service Unavailable",
                "We were unable to remove the item you requested, this failure has been reported.",
            )
        })?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TableStoragePage {
    #[serde(rename = "PartitionKey")]
    pub domain: String,
    #[serde(rename = "RowKey")]
    pub path: String,

    #[serde(rename = "Likes")]
    pub likes: u64,
    #[serde(rename = "Views")]
    pub views: u64,
}

impl From<TableStoragePage> for Page {
    fn from(entity: TableStoragePage) -> Self {
        Self {
            domain: entity.domain,
            path: entity.path,
            likes: entity.likes,
            views: entity.views,
        }
    }
}

trait AsyncHandler<M>
where
    M: Message,
{
    type Result;

    // This method is called for every message received by this actor.
    fn handle_internal(&self, msg: M) -> Pin<Box<dyn Future<Output = Self::Result>>>;
}

macro_rules! actor_handler {
    ($msg:ty => $res:ty: handler = $handler:item) => {

        impl AsyncHandler<$msg> for TableStorage {
            type Result = Result<$res, APIError>;

            $handler
        }

        impl actix::Handler<$msg> for TableStorage {
            type Result = ResponseActFuture<Self, Result<$res, APIError>>;

            fn handle(&mut self, msg: $msg, _ctx: &mut Self::Context) -> Self::Result {
                Box::pin(fut::wrap_future(self.handle_internal(msg)))
            }
        }

        impl actix::Handler<$crate::telemetry::TraceMessage<$msg>> for TableStorage {
            type Result = ResponseActFuture<Self, Result<$res, APIError>>;

            fn handle(&mut self, msg: $crate::telemetry::TraceMessage<$msg>, _ctx: &mut Self::Context) -> Self::Result {
                let work = self.handle_internal(msg.message);

                let instrumentation = work.instrument(msg.span);

                Box::pin(fut::wrap_future(instrumentation))
            }
        }
    };

    ($msg:ty|$src:ident => $res:ty: get_single from $table:ident ( $st:ty ) where pk=$pk:expr, rk=$rk:expr; not found = $err:expr) => {
        actor_handler!($msg => $res: handler = fn handle_internal(&self, $src: $msg) -> Pin<Box<dyn Future<Output = Self::Result>>> {
            let table = self.$table.clone();
            let work = TableStorage::get_single::<$st, $res>(
                table,
                "$table",
                $pk,
                $rk,
                APIError::new(404, "Not Found", $err));

            Box::pin(work)
        });
    };

    ($msg:ty|$src:ident => $res:ty: get_all from $table:ident ( $st:ty ) where query = $query:expr, context = [$($ctx:tt)*], filter = $fid:ident -> $filter:expr) => {
        actor_handler!($msg => Vec<$res>: handler = fn handle_internal(&self, $src: $msg) -> Pin<Box<dyn Future<Output = Self::Result>>> {
            let table = self.$table.clone();
            let query = $query;

            $($ctx)*

            let work = TableStorage::get_all::<$st, $res, _>(
                table,
                "$table",
                query,
                move |$fid| $filter
            );

            Box::pin(work)
        });
    };

    ($msg:ty|$src:ident => $res:ty: upsert_single in $table:ident ( $st:ty ) where pk=$pk:expr, rk=$rk:expr; default = $default:expr; update = $uid:ident -> $update:expr) => {
      actor_handler!($msg => $res: handler = fn handle_internal(&self, $src: $msg) -> Pin<Box<dyn Future<Output = Self::Result>>> {
        let table = self.$table.clone();
        let work = TableStorage::update_single::<$st, $res, fn(&mut $st) -> ()>(
          table,
          "$table",
          $pk,
          $rk,
          $default,
          move |$uid| $update
        );

        Box::pin(work)
      });
    };

    ($msg:ty|$src:ident: remove_single from $table:ident where pk=$pk:expr, rk=$rk:expr) => {
        actor_handler!($msg => (): handler = fn handle_internal(&self, $src: $msg) -> Pin<Box<dyn Future<Output = Self::Result>>> {
            let table = self.$table.clone();
            let work = TableStorage::remove_single(
                table,
                "$table",
                $pk,
                $rk);

            Box::pin(work)
        });
    };

    ($msg:ty|$src:ident => $res:ty: store_single in $table:ident ( $st:ty ) where pk=$pk:expr, rk=$rk:expr; return $item:expr) => {
        actor_handler!($msg => $res: handler = fn handle_internal(&self, $src: $msg) -> Pin<Box<dyn Future<Output = Self::Result>>> {
            let table = self.$table.clone();
            let item = $item;
            let work = TableStorage::store_single(
                table,
                "$table",
                $pk,
                $rk,
                item
            );

            Box::pin(work)
        });
    };
}

impl Actor for TableStorage {
    type Context = actix::prelude::Context<Self>;
}

trace_handler!(TableStorage, GetHealth, Result<Health, APIError>);

impl Handler<GetHealth> for TableStorage {
    type Result = Result<Health, APIError>;

    fn handle(&mut self, _: GetHealth, _: &mut Self::Context) -> Self::Result {
        Ok(Health {
            ok: true,
            started_at: self.started_at,
        })
    }
}

actor_handler!(GetPage|msg => Page: get_single from pages(TableStoragePage) where pk=msg.domain, rk=msg.path;
  not found = "The page you requested could not be found for this domain. Please check the page details and try again.");

actor_handler!(GetPages|msg => Page: get_all from pages(TableStoragePage) where
    query=format!("PartitionKey eq '{}'", msg.domain),
    context=[],
    filter=e -> e.domain == msg.domain);

actor_handler!(LikePage|msg => Page: upsert_single in pages(TableStoragePage) where pk=msg.domain.clone(), rk=msg.path.clone();
  default = TableStoragePage {
    domain: msg.domain.clone(),
    path: msg.path.clone(),
    likes: 0,
    views: 1,
  };
  update = page -> page.likes += 1);

actor_handler!(ViewPage|msg => Page: upsert_single in pages(TableStoragePage) where pk=msg.domain.clone(), rk=msg.path.clone();
  default = TableStoragePage {
    domain: msg.domain.clone(),
    path: msg.path.clone(),
    likes: 0,
    views: 0,
  };
  update = page -> page.views += 1);

// actor_handler!(StorePage|msg => Page: store_single in ideas(TableStoragePage) where pk=msg.domain.clone(), rk=msg.path.clone(); return TableStorageIdea {
//     domain: msg.domain,
//     path: msg.path,
//     likes: msg.views,
//     views: msg.views,
// });

//actor_handler!(RemovePage|msg: remove_single from ideas where pk=msg.domain, rk=msg.page);
