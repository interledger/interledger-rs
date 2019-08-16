# Documentation Guideline

Since we Interledger.rs developers place emphasis on realizing great developer experience so that newcomers could easily dive into the world of Interledger and Interledger.rs and hopefully integrate their applications with Interledger, documents on Interledger.rs MUST be understandable and crystal clear.

To provide such documents, here we define what we think are great documents. When Interledger.rs developers write documents, consider the following rules.

## Principles

The following is the essence of **great documents**. Details will be given later in each following section soon.

- Give a brief overview
- Define prerequisites
- Define terminology
- Structure explanations
    - A conclusion to reasons
    - List elements
    - Make the explanations MECE
- Give precise explanations
- Show structures using diagrams

### Give a Brief Overview
Giving a higher perspective is important because readers could catch what the writer wants to convey early on even though details might not be provided at that time. So you have to extract the essence of what you want to write first of all, and show it in an unstinting manner in the **Overview** section. Explanations which start with super details will give intolerable pain to the readers as they cannot understand what the writer wants to convey conclusively and for what reason they are taught those.

Then you have to follow up with **"Why is that?"** and **How to?** to make the image of the essence more concrete and precise.

### Define Prerequisites
An explanation is a pile of logics. Conversely, you have to be able to answer every question of **"Why so?"** and no middle logics should be curtailed. That said, writing all the parts is NOT realistic and there might be more suitable explanations elsewhere.

So you could expect the readers to have some particular knowledge which is necessary for your explanation. In the case, you have to clarify, in the **Prerequisites** section, which part will NOT be explained in your documents and give proper references where the readers could obtain the knowledge.

Keep in mind that readers might not be as well-informed as you. Even if you feel the explanation is sufficient FOR YOU, the readers might lack something significant to understand your explanation.

### Define Terminology
This is similar to prerequisites. If you use some technical terms which you think are very common, it is still worthy to define what they are in advance of explanations because the meanings which readers imagine might differ from yours or they might not even know the words.

It is not necessarily required to define the terms ON YOUR OWN. You could show references which are suitable to explain the intended meanings.

### Structure Explanations
Structuring explanations makes documents much more easy to understand. The following shows how to structure explanations.

- A conclusion to reasons
    - This is similar to the "Give a Brief Overview" section. "A big picture to details" is always a principle of explanations. If you have something conclusively you want to suggest, write it first of all.
- List elements
    - It is easy to grasp graphically what there are and how many items are repeated if it is listed just like this section. You could understand quickly 3 items are shown and these are in apposition.
- Make the explanations MECE
    - MECE means Mutually Exclusive and Collectively Exhaustive. You have to list essentially independent items which have no duplicated areas mutually. Also, you have to show those ALL with no omission.
    - Duplicated explanations make readers think "Aren't these the same?" which should be avoided. Left out items make readers think "What about this? Isn't this a part of?" which should be avoided.

### Give Precise Explanations
Because we provide technical documents, our documents MUST be precise. Being precise is being able **to make readers imagine the implementation** of what is explained.

The following is an example of being not precise.

> The node sends this information.
> - Integer
> - String

The example has so many unclear points that you could not even implement it.

- How does the node connect to the other nodes?
- The counterpart node is one or more?
- What is the format of the information?
- What do `Integer` and `String` mean? What are these for?

### Show Structures Using Diagrams
Understanding some concepts, especially complicated ones, with only words is difficult. It is because elements might be connected to each other and contained by another, which is hardly imagined sometimes. To explain these complicated ones, providing diagrams would be a solution.

## Options
According to the situation, the following elements might be helpful.

- Add a troubleshooting section
    - If you are writing examples or an explanation of procedures, adding a troubleshooting section might be useful for readers. For some reasons, scripts may fail. Providing common reasons and solutions just in case could save readers' time.
- Make it compatible with `run-md.sh`
    - If you are writing examples or an explanation of procedures, it is recommended to make it compatible with `run-md.sh`. Readers might not want to copy&paste all the procedures. Using `run-md.sh`, you can easily run all the scripts of a markdown page collectively. See [examples README](../examples/README.md).
