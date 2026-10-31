// @vitest-environment jsdom
import {cleanup,render,screen} from '@testing-library/react';
import {afterEach,expect,test} from 'vitest';
import {Markdown} from './Markdown';
afterEach(cleanup);
test('renders source Markdown as readable headings, lists, tables and code blocks',()=>{
 const {container}=render(<Markdown body={'# 学习笔记\n\n**重点**\n\n- 第一项\n- 第二项\n\n| 概念 | 定义 |\n| --- | --- |\n| 提交 | 生效 |\n\n```sql\nCOMMIT;\n```'}/>);
 expect(screen.getByRole('heading',{name:'学习笔记'})).toBeTruthy();expect(screen.getAllByRole('listitem')).toHaveLength(2);expect(screen.getByRole('table')).toBeTruthy();expect(container.querySelector('strong')?.textContent).toBe('重点');expect(container.querySelector('pre code')?.textContent).toContain('COMMIT');
});
test('untrusted source markup does not inject HTML or automatically load remote images',()=>{
 const {container}=render(<Markdown body={'<script>alert(1)</script>\n\n![远程图](https://example.com/track.png)\n\n[危险](javascript:alert(1))'}/>);
 expect(container.querySelector('script')).toBeNull();expect(container.querySelector('img')).toBeNull();expect(container.querySelector('[href^="javascript:"]')).toBeNull();expect(screen.getByText('[图片：远程图]')).toBeTruthy();
});
