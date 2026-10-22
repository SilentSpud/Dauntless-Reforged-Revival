use diesel::prelude::*;
use diesel::sql_types::Text;

use crate::{Connection, Error};

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    sql: String,
}

/// The schema as SQL, one `CREATE` statement per object: tables, then indexes, views and triggers,
/// each group by name. SQLite's own objects and the two migration-tracking tables are left out, so a
/// database built from a Diesel baseline and one Drizzle built give the same text (M18). The text
/// is valid SQL in creation order, so a baseline's `up.sql` can be exactly this.
pub fn schema_snapshot(conn: &mut Connection) -> Result<String, Error> {
    let rows = diesel::sql_query(
        "SELECT sql FROM sqlite_schema
         WHERE sql IS NOT NULL
           AND name NOT LIKE 'sqlite!_%' ESCAPE '!'
           AND tbl_name NOT IN ('__drizzle_migrations', '__diesel_schema_migrations')
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'view' THEN 2 ELSE 3 END, name",
    )
    .load::<Row>(conn)?;
    // Line endings as LF: a Windows checkout with CRLF files gave the stored statements CRLF.
    Ok(rows
        .into_iter()
        .map(|row| format!("{};\n", row.sql.replace("\r\n", "\n")))
        .collect())
}
