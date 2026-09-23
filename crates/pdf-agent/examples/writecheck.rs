use pdf_agent::desk::Desk;
use pdf_agent::json::Json;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(from), Some(to)) = (args.next(), args.next()) else {
        eprintln!("usage: writecheck <blank.pdf> <out.pdf>");
        return;
    };
    let asked = args
        .next()
        .map(|path| std::fs::read_to_string(path).expect("the markdown reads"));
    let theme = args.next();
    let example = "# สรุปการลงทุนที่น่าสนใจ ปี 2026\n\
        \n\
        ภาพรวมของปีที่จะถึงนี้ยังผสมกันอยู่ ดอกเบี้ยเริ่มลง แต่ความเสี่ยงด้านภูมิรัฐศาสตร์ยังสูง \
        นักลงทุนจึงควรกระจายความเสี่ยงมากกว่าไล่ตามผลตอบแทนระยะสั้น\n\
        \n\
        ## สินทรัพย์ที่น่าสนใจ\n\
        \n\
        - **พันธบัตรรัฐบาล** ได้ประโยชน์ตรงเมื่อดอกเบี้ยลง\n\
        - **ทองคำ** ยังทำหน้าที่ประกันความเสี่ยงได้ดี\n\
        - **หุ้นกลุ่มโครงสร้างพื้นฐาน** รายได้สม่ำเสมอ ทนเงินเฟ้อ\n\
        \n\
        ## สิ่งที่ควรระวัง\n\
        \n\
        1. อย่าลงทุนเกินกว่าที่รับความเสี่ยงไหว\n\
        2. ค่าธรรมเนียมกินผลตอบแทนมากกว่าที่คิด\n\
        3. ผลตอบแทนในอดีตไม่ได้บอกอนาคต\n\
        \n\
        > เอกสารนี้เป็นการสรุปเพื่อการศึกษา ไม่ใช่คำแนะนำการลงทุน\n\
        \n\
        ### หมายเหตุ\n\
        \n\
        ตัวเลขทั้งหมดเป็นการประมาณการ\n";
    let markdown = asked.as_deref().unwrap_or(example);
    let mut desk = Desk::default();
    let opened = pdf_agent::tools::call(
        &mut desk,
        "open_document",
        &Json::object([("path", Json::text(from))]),
    )
    .expect("the document opens");
    println!("{}", opened.text);
    let answer = pdf_agent::tools::call(
        &mut desk,
        "write_pages",
        &Json::object([
            ("document", Json::text("doc-1")),
            ("markdown", Json::text(markdown)),
            (
                "theme",
                Json::text(theme.unwrap_or_else(|| "classic".to_owned())),
            ),
        ]),
    );
    match answer {
        Ok(answer) => println!("{}", answer.text),
        Err(why) => {
            println!("refused: {why}");
            return;
        }
    }
    let saved = pdf_agent::tools::call(
        &mut desk,
        "save_document",
        &Json::object([("document", Json::text("doc-1")), ("path", Json::text(to))]),
    );
    match saved {
        Ok(answer) => println!("{}", answer.text),
        Err(why) => println!("not saved: {why}"),
    }
}
