use std::{collections::HashMap, fs, path::Path, sync::Arc};

use parquet::{
    basic::Compression,
    data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type},
    file::{properties::WriterProperties, writer::SerializedFileWriter},
    schema::parser::parse_message_type,
};

use crate::{CampaignDay, Result, UserDay};

const USER_SCHEMA: &str = "message user_daily {
    REQUIRED BINARY user_id (UTF8);
    REQUIRED INT32 day (DATE);
    REQUIRED INT64 sessions;
    REQUIRED INT64 events;
    REQUIRED INT64 revenue_cents;
    OPTIONAL BINARY top_category (UTF8);
}";

const CAMPAIGN_SCHEMA: &str = "message campaign_daily {
    REQUIRED BINARY campaign_id (UTF8);
    REQUIRED BINARY channel (UTF8);
    REQUIRED INT32 day (DATE);
    REQUIRED INT64 sessions;
    REQUIRED INT64 purchasing_sessions;
    REQUIRED INT64 revenue_cents;
    REQUIRED INT64 assisted_sessions;
}";

enum Column<'a> {
    Text(&'a [ByteArray]),
    OptionalText(&'a [ByteArray], &'a [i16]),
    Date(&'a [i32]),
    Count(&'a [i64]),
}

fn writer(file: &Path, schema: &str) -> Result<SerializedFileWriter<fs::File>> {
    let schema = Arc::new(parse_message_type(schema)?);
    let properties = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build(),
    );
    Ok(SerializedFileWriter::new(
        fs::File::create(file)?,
        schema,
        properties,
    )?)
}

fn write_group(writer: &mut SerializedFileWriter<fs::File>, columns: &[Column<'_>]) -> Result<()> {
    let mut group = writer.next_row_group()?;
    for values in columns {
        let mut column = group
            .next_column()?
            .expect("column exists in output schema");
        match values {
            Column::Text(values) => column
                .typed::<ByteArrayType>()
                .write_batch(values, None, None)?,
            Column::OptionalText(values, levels) => {
                column
                    .typed::<ByteArrayType>()
                    .write_batch(values, Some(levels), None)?
            }
            Column::Date(values) => column
                .typed::<Int32Type>()
                .write_batch(values, None, None)?,
            Column::Count(values) => column
                .typed::<Int64Type>()
                .write_batch(values, None, None)?,
        };
        column.close()?;
    }
    group.close()?;
    Ok(())
}

pub struct UserWriter {
    writer: SerializedFileWriter<fs::File>,
    ids: Vec<ByteArray>,
    days: Vec<i32>,
    sessions: Vec<i64>,
    events: Vec<i64>,
    revenue: Vec<i64>,
    categories: Vec<ByteArray>,
    category_levels: Vec<i16>,
}

impl UserWriter {
    pub fn new(destination: &Path, partition: usize) -> Result<Self> {
        let file = destination
            .join("user_daily")
            .join(format!("part-{partition:03}.parquet"));
        Ok(Self {
            writer: writer(&file, USER_SCHEMA)?,
            ids: Vec::new(),
            days: Vec::new(),
            sessions: Vec::new(),
            events: Vec::new(),
            revenue: Vec::new(),
            categories: Vec::new(),
            category_levels: Vec::new(),
        })
    }

    pub fn push(
        &mut self,
        user: &[u8],
        day: i32,
        totals: UserDay,
        category: Option<&[u8]>,
    ) -> Result<()> {
        self.ids.push(ByteArray::from(user.to_vec()));
        self.days.push(day);
        self.sessions.push(totals.sessions);
        self.events.push(totals.events);
        self.revenue.push(totals.revenue);
        self.category_levels.push(i16::from(category.is_some()));
        if let Some(category) = category {
            self.categories.push(ByteArray::from(category.to_vec()));
        }
        if self.ids.len() >= 32_768 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.ids.is_empty() {
            return Ok(());
        }
        write_group(
            &mut self.writer,
            &[
                Column::Text(&self.ids),
                Column::Date(&self.days),
                Column::Count(&self.sessions),
                Column::Count(&self.events),
                Column::Count(&self.revenue),
                Column::OptionalText(&self.categories, &self.category_levels),
            ],
        )?;
        self.ids.clear();
        self.days.clear();
        self.sessions.clear();
        self.events.clear();
        self.revenue.clear();
        self.categories.clear();
        self.category_levels.clear();
        Ok(())
    }

    pub fn close(mut self) -> Result<()> {
        self.flush()?;
        self.writer.close()?;
        Ok(())
    }
}

pub fn write_campaigns(
    destination: &Path,
    campaigns: &[(&[u8], &[u8])],
    totals: HashMap<(usize, i32), CampaignDay>,
) -> Result<()> {
    let directory = destination.join("campaign_daily");
    fs::create_dir_all(&directory)?;
    let mut writer = writer(&directory.join("part-000.parquet"), CAMPAIGN_SCHEMA)?;
    if !totals.is_empty() {
        let (mut ids, mut channels, mut days) = (Vec::new(), Vec::new(), Vec::new());
        let (mut sessions, mut purchasing, mut revenue, mut assists) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for ((campaign, day), values) in totals {
            let (id, channel) = campaigns[campaign];
            ids.push(ByteArray::from(id.to_vec()));
            channels.push(ByteArray::from(channel.to_vec()));
            days.push(day);
            sessions.push(values.sessions);
            purchasing.push(values.purchasing);
            revenue.push(values.revenue);
            assists.push(values.assists);
        }
        write_group(
            &mut writer,
            &[
                Column::Text(&ids),
                Column::Text(&channels),
                Column::Date(&days),
                Column::Count(&sessions),
                Column::Count(&purchasing),
                Column::Count(&revenue),
                Column::Count(&assists),
            ],
        )?;
    }
    writer.close()?;
    Ok(())
}
