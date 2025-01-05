#[macro_export]
macro_rules! test_state {
    (:: $state:ident = []) => (());
    (:: $state:ident = [ $item:expr ]) => {
        $state.store.send($item).await.expect("the actor should be run").expect("the operation should succeed");
    };

    (:: $state:ident = [ $item:expr, $($rest:expr),* ]) => {
        test_state!(:: $state = [ $item ]);
        test_state!(:: $state = [ $($rest),* ]);
    };

    ($state:ident = [ $($init:expr),* ]) => {
        let $state = $crate::models::GlobalState::new(":memory:").unwrap();

        test_state!(:: $state = [ $($init),* ]);
    }
}

#[macro_export]
macro_rules! test_request {

    ($method:ident $path:expr => $status:ident | state = $state:ident) => {
        {
            let app = $crate::api::test::get_test_app($state.clone()).await;
            let req = actix_web::test::TestRequest::with_uri($path)
                .method(http::Method::$method)
                .insert_header(("User-Agent", "Test"))
                .to_request();

            let response = actix_web::test::call_service(&app, req).await;
            $crate::api::test::assert_status(response, http::StatusCode::$status).await
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident | state = $state:ident) => {
        {
            let app = $crate::api::test::get_test_app($state.clone()).await;
            let req = actix_web::test::TestRequest::with_uri($path)
                .method(http::Method::$method)
                .set_json(&$body)
                .insert_header(("User-Agent", "Test"))
                .to_request();

            let response = actix_web::test::call_service(&app, req).await;
            $crate::api::test::assert_status(response, http::StatusCode::$status).await
        }
    };

    ($method:ident $path:expr => $status:ident with content | state = $state:ident) => {
        {
            let response = test_request!($method $path => $status | state = $state);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident with content | state = $state:ident) => {
        {
            let response = test_request!($method $path, $body => $status | state = $state);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr => $status:ident with location =~ $location:expr, content | state = $state:ident) => {
        {
            let response = test_request!($method $path => $status | state = $state);
            $crate::api::test::assert_location_header(response.headers(), $location);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident with location =~ $location:expr, content | state = $state:ident) => {
        {
            let response = test_request!($method $path, $body => $status | state = $state);
            $crate::api::test::assert_location_header(response.headers(), $location);
            $crate::api::test::get_content(response).await
        }
    };

    /* --------------- NO GLOBAL STATE ------------------ */

    ($method:ident $path:expr => $status:ident) => {
        {
            let state = $crate::models::GlobalState::new(":memory:").unwrap();

            test_request!($method $path => $status | state = state)
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident) => {
        {
            let state = $crate::models::GlobalState::new(":memory:").unwrap();

            test_request!($method $path, $body => $status | state = state)
        }
    };

    ($method:ident $path:expr => $status:ident with content) => {
        {
            let response = test_request!($method $path => $status);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident with content) => {
        {
            let response = test_request!($method $path, $body => $status);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr => $status:ident with location =~ $location:expr, content) => {
        {
            let response = test_request!($method $path => $status);
            assert_location_header(response.headers(), $location);
            $crate::api::test::get_content(response).await
        }
    };

    ($method:ident $path:expr, $body:expr => $status:ident with location =~ $location:expr, content) => {
        {
            let response = test_request!($method $path, $body => $status);
            assert_location_header(response.headers(), $location);
            $crate::api::test::get_content(response).await
        }
    };
}
