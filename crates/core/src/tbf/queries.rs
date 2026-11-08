//! GraphQL operation strings copied verbatim from the Web API implementation
//! (packages/ligrol-api/src/routes/{books,samples,events}.ts and
//! src/lib/checklist-graphql.ts). Do not reformat the query text.

/// books.ts:462 — verbatim.
pub(crate) const BOOKSHELF_QUERY: &str = "query BookShelfQuery($first: Int!, $after: String) { viewer { id bookShelfItems(first: $first, after: $after) { pageInfo { hasNextPage endCursor } edges { node { id causedAt product { databaseID name organization { name } coverImage { url } downloadContent { fileName downloadURL } } marketHandshake { event { id name } } } cursor } } } }";

/// checklist の `EventOfflineCircleChecklistQuery`。
///
/// 2026-11-04 の Web バンドルから採取した現在の形に合わせる。旧クエリは
/// `ProductInfo.firstAppearanceEventName` と `Event.databaseID` を要求していたが、
/// 現行スキーマの checklist フラグメントはどちらも使わない（認証済み viewer で
/// 実行すると GraphQL errors になり、同期が「upstream error」で失敗していた）。
/// `followingOrganizations` は `@skip` で取得しない（チェックリスト表示には不要）。
pub(crate) const CHECKLIST_QUERY: &str = r#"
    query EventOfflineCircleChecklistQuery(
      $checkedProductInfosFirst: Int!
      $checkedProductInfosAfter: String
      $checkedProductInfosSkip: Boolean! = false
      $followingOrganizationsFirst: Int!
      $followingOrganizationsAfter: String
      $followingOrganizationsSkip: Boolean! = false
      $eventID: ID!
    ) {
      viewer {
        id
        checkedProductInfos(
          first: $checkedProductInfosFirst
          after: $checkedProductInfosAfter
        ) @skip(if: $checkedProductInfosSkip) {
          pageInfo { hasNextPage endCursor }
          edges {
            node {
              id
              createdAt
              productInfo {
                id
                databaseID
                name
                loginUserBookShelfItem { id }
                coverImage { url }
                productVariants(first: 10) {
                  edges {
                    node {
                      id
                      kind
                      price
                      status
                    }
                  }
                }
                organization {
                  id
                  name
                  circles(first: 1, criteria: {eventID: $eventID}) {
                    pageInfo { hasNextPage endCursor }
                    edges {
                      node {
                        id
                        databaseID
                        spaces
                        hasOfflineCourse
                        event { id }
                      }
                      cursor
                    }
                  }
                }
              }
            }
            cursor
          }
        }
        followingOrganizations(
          first: $followingOrganizationsFirst
          after: $followingOrganizationsAfter
        ) @skip(if: $followingOrganizationsSkip) {
          pageInfo { hasNextPage endCursor }
          edges {
            cursor
            node {
              id
              organization {
                id
                name
                image { id url width height }
                circles(first: 1, criteria: {eventID: $eventID}) {
                  pageInfo { hasNextPage endCursor }
                  edges {
                    node {
                      id
                      databaseID
                      spaces
                      hasOfflineCourse
                      event { id }
                    }
                    cursor
                  }
                }
              }
            }
          }
        }
      }
      event(id: $eventID) {
        id
        recruitStartAt
      }
    }
"#;

/// samples.ts:97-115 — verbatim.
pub(crate) const PRODUCT_IMAGES_QUERY: &str = r#"
    query ProductImagesQuery($productInfoID: ID!) {
      product(id: $productInfoID) {
        id
        databaseID
        images(first: 8) {
          edges {
            node {
              id
              databaseID
              url
              width
              height
            }
          }
        }
      }
    }
"#;

/// events.ts:96 `buildGraphQLQuery()` — verbatim.
pub(crate) const EVENT_QUERY: &str =
    "query TbfEventQuery($eventID: ID!) { event(id: $eventID) { id databaseID name } }";

/// Captured live on 2026-08-21 (browser): the login mutation the
/// techbookfest.org SPA itself sends.
pub(crate) const LOGIN_MUTATION: &str = "mutation UserLoginMutation($loginInput: LoginUserInput!) { loginUser(input: $loginInput) { user { id ...GetMeUserFragment __typename } __typename } } fragment GetMeUserFragment on User { id email __typename }";
